use capnp::capability::Promise;
use capnp_rpc::rpc_capnp::{
    call, cap_descriptor, disembargo, message, message_target, payload, promised_answer, resolve,
    return_,
};
use capnp_rpc::{Connection, FlowController, IncomingMessage, OutgoingMessage, VatNetwork};
use futures::FutureExt;
use std::marker::PhantomData;
use std::rc::Rc;

pub type RejectionObserver = Rc<dyn Fn(&str)>;

fn check_descriptor(descriptor: cap_descriptor::Reader) -> Result<(), String> {
    match descriptor.which() {
        Ok(cap_descriptor::None(()))
        | Ok(cap_descriptor::SenderHosted(_))
        | Ok(cap_descriptor::SenderPromise(_))
        | Ok(cap_descriptor::ReceiverHosted(_)) => Ok(()),
        Ok(cap_descriptor::ReceiverAnswer(answer)) => {
            check_promised_answer(answer.map_err(|_| "capDescriptor.receiverAnswer".to_string())?)
        }
        Ok(cap_descriptor::ThirdPartyHosted(_)) => Err("capDescriptor.thirdPartyHosted".into()),
        Err(capnp::NotInSchema(value)) => Err(format!("capDescriptor.notInSchema({value})")),
    }
}

fn check_promised_answer(answer: promised_answer::Reader) -> Result<(), String> {
    let transform = answer
        .get_transform()
        .map_err(|_| "promisedAnswer.transform".to_string())?;
    for operation in transform {
        if let Err(capnp::NotInSchema(value)) = operation.which() {
            return Err(format!("promisedAnswer.op.notInSchema({value})"));
        }
    }
    Ok(())
}

fn check_target(
    target: capnp::Result<message_target::Reader>,
    context: &str,
) -> Result<(), String> {
    let target = target.map_err(|_| format!("{context}.target"))?;
    match target.which() {
        Ok(message_target::ImportedCap(_)) => Ok(()),
        Ok(message_target::PromisedAnswer(answer)) => {
            check_promised_answer(answer.map_err(|_| format!("{context}.target.promisedAnswer"))?)
        }
        Err(capnp::NotInSchema(value)) => Err(format!("{context}.target.notInSchema({value})")),
    }
}

fn check_payload(payload: capnp::Result<payload::Reader>, context: &str) -> Result<(), String> {
    let payload = payload.map_err(|_| format!("{context}.payload"))?;
    let cap_table = payload
        .get_cap_table()
        .map_err(|_| format!("{context}.capTable"))?;
    for descriptor in cap_table {
        check_descriptor(descriptor)?;
    }
    Ok(())
}

pub fn check_message(reader: message::Reader) -> Result<(), String> {
    match reader.which() {
        Ok(message::Unimplemented(inner)) => {
            check_message(inner.map_err(|_| "unimplemented".to_string())?)
                .map_err(|kind| format!("unimplemented({kind})"))
        }
        Ok(message::Abort(_))
        | Ok(message::Bootstrap(_))
        | Ok(message::Finish(_))
        | Ok(message::Release(_)) => Ok(()),
        Ok(message::Call(call)) => {
            let call = call.map_err(|_| "call".to_string())?;
            match call.get_send_results_to().which() {
                Ok(call::send_results_to::Caller(())) | Ok(call::send_results_to::Yourself(())) => {
                }
                Ok(call::send_results_to::ThirdParty(_)) => {
                    return Err("call.sendResultsTo.thirdParty".into());
                }
                Err(capnp::NotInSchema(value)) => {
                    return Err(format!("call.sendResultsTo.notInSchema({value})"));
                }
            }
            check_target(call.get_target(), "call")?;
            check_payload(call.get_params(), "call")
        }
        Ok(message::Return(answer)) => {
            let answer = answer.map_err(|_| "return".to_string())?;
            match answer.which() {
                Ok(return_::Results(results)) => check_payload(results, "return"),
                Ok(return_::Exception(_))
                | Ok(return_::Canceled(()))
                | Ok(return_::ResultsSentElsewhere(()))
                | Ok(return_::TakeFromOtherQuestion(_)) => Ok(()),
                Ok(return_::AcceptFromThirdParty(_)) => Err("return.acceptFromThirdParty".into()),
                Err(capnp::NotInSchema(value)) => Err(format!("return.notInSchema({value})")),
            }
        }
        Ok(message::Resolve(resolution)) => {
            let resolution = resolution.map_err(|_| "resolve".to_string())?;
            match resolution.which() {
                Ok(resolve::Cap(descriptor)) => {
                    check_descriptor(descriptor.map_err(|_| "resolve.cap".to_string())?)
                }
                Ok(resolve::Exception(_)) => Ok(()),
                Err(capnp::NotInSchema(value)) => Err(format!("resolve.notInSchema({value})")),
            }
        }
        Ok(message::Disembargo(embargo)) => {
            let embargo = embargo.map_err(|_| "disembargo".to_string())?;
            match embargo.get_context().which() {
                Ok(disembargo::context::SenderLoopback(_))
                | Ok(disembargo::context::ReceiverLoopback(_)) => {}
                Ok(disembargo::context::Accept(())) => {
                    return Err("disembargo.context.accept".into());
                }
                Ok(disembargo::context::Provide(_)) => {
                    return Err("disembargo.context.provide".into());
                }
                Err(capnp::NotInSchema(value)) => {
                    return Err(format!("disembargo.context.notInSchema({value})"));
                }
            }
            check_target(embargo.get_target(), "disembargo")
        }
        Ok(message::Provide(_)) => Err("provide".into()),
        Ok(message::Accept(_)) => Err("accept".into()),
        Ok(message::Join(_)) => Err("join".into()),
        Ok(message::ObsoleteSave(_)) => Err("obsoleteSave".into()),
        Ok(message::ObsoleteDelete(_)) => Err("obsoleteDelete".into()),
        Err(capnp::NotInSchema(value)) => Err(format!("notInSchema({value})")),
    }
}

fn check_incoming(incoming: &dyn IncomingMessage) -> Result<(), String> {
    let body = incoming.get_body().map_err(|_| "unreadable".to_string())?;
    let reader: message::Reader = body.get_as().map_err(|_| "unreadable".to_string())?;
    check_message(reader)
}

struct FilteredConnection<VatId> {
    inner: Box<dyn Connection<VatId>>,
    on_rejected: RejectionObserver,
}

impl<VatId: 'static> Connection<VatId> for FilteredConnection<VatId> {
    fn get_peer_vat_id(&self) -> VatId {
        self.inner.get_peer_vat_id()
    }

    fn new_outgoing_message(&mut self, first_segment_word_size: u32) -> Box<dyn OutgoingMessage> {
        self.inner.new_outgoing_message(first_segment_word_size)
    }

    fn receive_incoming_message(
        &mut self,
    ) -> Promise<Option<Box<dyn IncomingMessage>>, capnp::Error> {
        let on_rejected = self.on_rejected.clone();
        Promise::from_future(self.inner.receive_incoming_message().map(move |received| {
            let incoming = received?;
            if let Some(message) = &incoming
                && let Err(kind) = check_incoming(message.as_ref())
            {
                tracing::error!(kind = %kind, "rejected an rpc message and aborted the connection");
                on_rejected(&kind);
                return Err(capnp::Error::failed(format!(
                    "rejected rpc message: {kind}"
                )));
            }
            Ok(incoming)
        }))
    }

    fn new_stream(&mut self) -> (Box<dyn FlowController>, Promise<(), capnp::Error>) {
        self.inner.new_stream()
    }

    fn shutdown(&mut self, result: capnp::Result<()>) -> Promise<(), capnp::Error> {
        self.inner.shutdown(result)
    }
}

pub struct FilteredVatNetwork<VatId, Network> {
    inner: Network,
    on_rejected: RejectionObserver,
    marker: PhantomData<VatId>,
}

impl<VatId: 'static, Network: VatNetwork<VatId>> VatNetwork<VatId>
    for FilteredVatNetwork<VatId, Network>
{
    fn connect(&mut self, host_id: VatId) -> Option<Box<dyn Connection<VatId>>> {
        let on_rejected = self.on_rejected.clone();
        self.inner.connect(host_id).map(|inner| {
            Box::new(FilteredConnection { inner, on_rejected }) as Box<dyn Connection<VatId>>
        })
    }

    fn accept(&mut self) -> Promise<Box<dyn Connection<VatId>>, capnp::Error> {
        let on_rejected = self.on_rejected.clone();
        Promise::from_future(self.inner.accept().map(move |accepted| {
            accepted.map(|inner| {
                Box::new(FilteredConnection { inner, on_rejected }) as Box<dyn Connection<VatId>>
            })
        }))
    }

    fn drive_until_shutdown(&mut self) -> Promise<(), capnp::Error> {
        self.inner.drive_until_shutdown()
    }
}

pub fn filter_vat_network<VatId: 'static, Network: VatNetwork<VatId>>(
    network: Network,
    on_rejected: impl Fn(&str) + 'static,
) -> FilteredVatNetwork<VatId, Network> {
    FilteredVatNetwork {
        inner: network,
        on_rejected: Rc::new(on_rejected),
        marker: PhantomData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp::message::{Builder, HeapAllocator, ReaderOptions};
    use capnp_rpc::rpc_twoparty_capnp::Side;
    use capnp_rpc::twoparty;
    use std::cell::RefCell;

    fn build(fill: impl FnOnce(message::Builder)) -> Builder<HeapAllocator> {
        let mut builder = Builder::new_default();
        fill(builder.init_root());
        builder
    }

    fn check(builder: &Builder<HeapAllocator>) -> Result<(), String> {
        check_message(builder.get_root_as_reader().unwrap())
    }

    fn with_descriptor(fill: impl Fn(cap_descriptor::Builder)) -> Vec<Builder<HeapAllocator>> {
        vec![
            build(|root| {
                let mut call = root.init_call();
                call.reborrow().init_target().set_imported_cap(0);
                fill(call.init_params().init_cap_table(1).get(0));
            }),
            build(|root| {
                let answer = root.init_return();
                fill(answer.init_results().init_cap_table(1).get(0));
            }),
            build(|root| fill(root.init_resolve().init_cap())),
        ]
    }

    #[test]
    fn allows_every_level_one_message() {
        let allowed = [
            build(|root| {
                root.init_abort();
            }),
            build(|root| {
                root.init_bootstrap();
            }),
            build(|root| {
                root.init_finish();
            }),
            build(|root| {
                root.init_release();
            }),
            build(|root| {
                let mut call = root.init_call();
                call.reborrow().get_send_results_to().set_caller(());
                let mut target = call.reborrow().init_target().init_promised_answer();
                target
                    .reborrow()
                    .init_transform(2)
                    .get(1)
                    .set_get_pointer_field(0);
                call.init_params();
            }),
            build(|root| {
                let mut call = root.init_call();
                call.reborrow().get_send_results_to().set_yourself(());
                call.reborrow().init_target().set_imported_cap(3);
            }),
            build(|root| {
                root.init_return().init_exception();
            }),
            build(|root| root.init_return().set_canceled(())),
            build(|root| root.init_return().set_results_sent_elsewhere(())),
            build(|root| root.init_return().set_take_from_other_question(7)),
            build(|root| {
                root.init_resolve().init_exception();
            }),
            build(|root| {
                let mut embargo = root.init_disembargo();
                embargo.reborrow().get_context().set_sender_loopback(1);
                embargo.init_target().set_imported_cap(0);
            }),
            build(|root| {
                let mut embargo = root.init_disembargo();
                embargo.reborrow().get_context().set_receiver_loopback(1);
                embargo.init_target().set_imported_cap(0);
            }),
            build(|root| {
                let inner = root.init_unimplemented();
                inner.init_call().init_target().set_imported_cap(0);
            }),
        ];
        for message in &allowed {
            assert_eq!(check(message), Ok(()));
        }
        for fill in [
            (|mut descriptor: cap_descriptor::Builder| descriptor.set_none(()))
                as fn(cap_descriptor::Builder),
            |mut descriptor| descriptor.set_sender_hosted(1),
            |mut descriptor| descriptor.set_sender_promise(1),
            |mut descriptor| descriptor.set_receiver_hosted(1),
            |descriptor| {
                let mut answer = descriptor.init_receiver_answer();
                answer.set_question_id(1);
                answer.init_transform(1).get(0).set_noop(());
            },
        ] {
            for message in with_descriptor(fill) {
                assert_eq!(check(&message), Ok(()));
            }
        }
    }

    #[test]
    fn rejects_every_message_beyond_level_one() {
        let rejected = [
            (
                build(|root| {
                    root.init_provide();
                }),
                "provide",
            ),
            (
                build(|root| {
                    root.init_accept();
                }),
                "accept",
            ),
            (
                build(|root| {
                    root.init_join();
                }),
                "join",
            ),
            (
                build(|root| {
                    root.init_obsolete_save();
                }),
                "obsoleteSave",
            ),
            (
                build(|root| {
                    root.init_obsolete_delete();
                }),
                "obsoleteDelete",
            ),
            (
                build(|root| {
                    let mut call = root.init_call();
                    call.reborrow().init_target().set_imported_cap(0);
                    call.get_send_results_to().init_third_party();
                }),
                "call.sendResultsTo.thirdParty",
            ),
            (
                build(|root| {
                    root.init_return().init_accept_from_third_party();
                }),
                "return.acceptFromThirdParty",
            ),
            (
                build(|root| {
                    let mut embargo = root.init_disembargo();
                    embargo.reborrow().get_context().set_accept(());
                    embargo.init_target().set_imported_cap(0);
                }),
                "disembargo.context.accept",
            ),
            (
                build(|root| {
                    let mut embargo = root.init_disembargo();
                    embargo.reborrow().get_context().set_provide(1);
                    embargo.init_target().set_imported_cap(0);
                }),
                "disembargo.context.provide",
            ),
            (
                build(|root| {
                    root.init_unimplemented().init_provide();
                }),
                "unimplemented(provide)",
            ),
        ];
        for (message, kind) in &rejected {
            assert_eq!(check(message), Err(kind.to_string()));
        }
        for message in with_descriptor(|descriptor| {
            descriptor.init_third_party_hosted();
        }) {
            assert_eq!(
                check(&message),
                Err("capDescriptor.thirdPartyHosted".to_string())
            );
        }
    }

    fn unknown_message() -> Vec<u8> {
        let builder = build(|root| {
            root.init_abort();
        });
        let mut bytes: Vec<u8> = builder.get_segments_for_output()[0].to_vec();
        bytes[8] = 99;
        bytes[9] = 0;
        bytes
    }

    #[test]
    fn rejects_kinds_the_schema_does_not_know() {
        let bytes = unknown_message();
        let segments: &[&[u8]] = &[&bytes];
        let reader = capnp::message::Reader::new(
            capnp::message::SegmentArray::new(segments),
            ReaderOptions::new(),
        );
        assert_eq!(
            check_message(reader.get_root().unwrap()),
            Err("notInSchema(99)".to_string())
        );
    }

    fn wire(message: &Builder<HeapAllocator>) -> Vec<u8> {
        let mut bytes = Vec::new();
        capnp::serialize::write_message(&mut bytes, message).unwrap();
        bytes
    }

    fn network(bytes: Vec<u8>, side: Side) -> twoparty::VatNetwork<futures::io::Cursor<Vec<u8>>> {
        twoparty::VatNetwork::new(
            futures::io::Cursor::new(bytes),
            futures::io::Cursor::new(Vec::new()),
            side,
            ReaderOptions::new(),
        )
    }

    #[test]
    fn aborts_accepted_and_connected_connections_on_a_rejected_message() {
        let provide = wire(&build(|root| {
            root.init_provide();
        }));
        let mut legitimate = wire(&build(|root| {
            root.init_bootstrap();
        }));
        legitimate.extend_from_slice(&provide);
        let seen = Rc::new(RefCell::new(Vec::new()));

        let observer = seen.clone();
        let mut accepting =
            filter_vat_network(network(legitimate, Side::Server), move |kind: &str| {
                observer.borrow_mut().push(kind.to_string())
            });
        let mut connection = futures::executor::block_on(accepting.accept())
            .ok()
            .unwrap();
        let first = futures::executor::block_on(connection.receive_incoming_message());
        assert!(matches!(first, Ok(Some(_))));
        let second = futures::executor::block_on(connection.receive_incoming_message());
        let error = second.err().unwrap();
        assert_eq!(error.kind, capnp::ErrorKind::Failed);
        assert!(
            error.extra.contains("rejected rpc message: provide"),
            "{}",
            error.extra
        );

        let observer = seen.clone();
        let mut connecting =
            filter_vat_network(network(provide, Side::Client), move |kind: &str| {
                observer.borrow_mut().push(kind.to_string())
            });
        let mut connection = connecting.connect(Side::Server).unwrap();
        let received = futures::executor::block_on(connection.receive_incoming_message());
        assert!(received.is_err());
        assert_eq!(*seen.borrow(), ["provide", "provide"]);
    }
}
