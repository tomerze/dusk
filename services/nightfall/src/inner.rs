use crate::audit::{SessionFacts, detail, session_event};
use crate::directory::namespace_hex;
use crate::events::now;
use crate::peer::PrincipalCertificate;
use crate::session::{InnerClient, NodeSession, SessionLink};
use crate::shard::Shard;
use capnp::any_pointer;
use capnp::capability::{
    DispatchCallResult, FromClientHook, FromServer, Params, Promise, Results, Server,
};
use capnp::private::capability::ClientHook;
use capnp_rpc::rpc_twoparty_capnp::Side;
use nightfall_ledger::entry::{EntryContent, Event};
use nightfall_membrane::membrane::Membrane;
use serde_json::Value;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub const NOT_CONNECTED_LIFETIME: Duration = Duration::from_secs(60);
pub const DISCONNECT_GRACE: Duration = Duration::from_secs(1);

pub fn not_connected_error(namespace_id: u64) -> capnp::Error {
    capnp::Error::disconnected(format!(
        "node {} is not connected",
        namespace_hex(namespace_id)
    ))
}

pub struct NotConnected {
    namespace_id: u64,
}

impl Server for Box<NotConnected> {
    fn dispatch_call(
        &mut self,
        _interface_id: u64,
        _method_id: u16,
        _params: Params<any_pointer::Owned>,
        _results: Results<any_pointer::Owned>,
    ) -> DispatchCallResult {
        DispatchCallResult::new(Promise::err(not_connected_error(self.namespace_id)), false)
    }
}

pub struct NotConnectedClient {
    hook: Box<dyn ClientHook>,
}

impl FromClientHook for NotConnectedClient {
    fn new(hook: Box<dyn ClientHook>) -> NotConnectedClient {
        NotConnectedClient { hook }
    }

    fn into_client_hook(self) -> Box<dyn ClientHook> {
        self.hook
    }

    fn as_client_hook(&self) -> &dyn ClientHook {
        &*self.hook
    }
}

impl FromServer<NotConnected> for NotConnectedClient {
    type Dispatch = Box<NotConnected>;

    fn from_server(server: NotConnected) -> Box<NotConnected> {
        Box::new(server)
    }
}

pub fn not_connected_bootstrap(namespace_id: u64) -> capnp::capability::Client {
    let client: NotConnectedClient = capnp_rpc::new_client(NotConnected { namespace_id });
    capnp::capability::Client::new(client.hook)
}

pub async fn serve_not_connected<Stream>(
    shard: Rc<Shard>,
    stream: Stream,
    namespace_id: u64,
    principal: String,
    remote: SocketAddr,
) where
    Stream: AsyncRead + AsyncWrite + Unpin + 'static,
{
    tracing::info!(
        namespace_id = %namespace_hex(namespace_id),
        %remote,
        "a client asked for a node nobody holds"
    );
    let audit = shard.audit.clone();
    let system = crate::rpc::system(
        stream,
        Side::Server,
        shard.shared.limits.max_message_bytes,
        Some(not_connected_bootstrap(namespace_id)),
        move |kind: &str| {
            tracing::error!(
                principal = %principal,
                namespace_id = %namespace_hex(namespace_id),
                %remote,
                kind,
                "a client sent a rejected rpc message; the link is closed"
            );
            let mut content = EntryContent::event(Event::RpcRejected, &principal);
            content.namespace_id = Some(namespace_id);
            content.event_detail = Some(detail(&[
                ("kind", Value::String(kind.to_string())),
                ("remote_address", Value::String(remote.to_string())),
            ]));
            audit.record_event(content);
        },
    );
    match tokio::time::timeout(NOT_CONNECTED_LIFETIME, system).await {
        Ok(Ok(())) | Err(_) => {}
        Ok(Err(error)) => tracing::debug!(%remote, %error, "a not-connected client link failed"),
    }
}

pub fn refuse_principal(
    shard: &Shard,
    principal: &PrincipalCertificate,
    namespace_id: u64,
    remote: SocketAddr,
) -> bool {
    let permissions = shard.shared.permissions.load();
    let refused = if permissions.denies_certificate(&principal.fingerprint_bytes) {
        Some("its certificate is denied")
    } else if permissions.denies_principal(&principal.principal) {
        Some("the principal is denied")
    } else {
        None
    };
    if let Some(reason) = refused {
        metrics::counter!("nightfall_principals_refused_total").increment(1);
        tracing::warn!(
            principal = %principal.principal,
            fingerprint = %principal.fingerprint,
            namespace_id = %namespace_hex(namespace_id),
            %remote,
            reason,
            "refused an inner client"
        );
        return true;
    }
    false
}

pub async fn serve_client<Stream>(
    shard: Rc<Shard>,
    session: Rc<NodeSession>,
    stream: Stream,
    principal: PrincipalCertificate,
    remote: SocketAddr,
) where
    Stream: AsyncRead + AsyncWrite + Unpin + 'static,
{
    if refuse_principal(&shard, &principal, session.namespace_id, remote) {
        return;
    }
    let permissions = shard.shared.permissions.load();
    let Some(policy) = permissions.policy_for(&principal.principal, session.quarantined.get())
    else {
        metrics::counter!("nightfall_principals_refused_total").increment(1);
        tracing::warn!(
            principal = %principal.principal,
            namespace_id = %namespace_hex(session.namespace_id),
            %remote,
            "refused an inner client: the principal has no role"
        );
        return;
    };
    let membrane = Membrane::new(
        SessionLink::new(&session),
        principal.principal.clone(),
        policy,
        session.bundle.clone(),
        shard.audit.clone(),
        shard.param_key.clone(),
        shard.limits.clone(),
    );
    let session_id = membrane.session_id().to_string();
    session.add_client(Rc::new(InnerClient {
        membrane: membrane.clone(),
        principal: principal.principal.clone(),
        fingerprint: principal.fingerprint_bytes,
        remote,
        connected_at: now(),
    }));
    let audit = shard.audit.clone();
    let device_id = session.device_id().to_string();
    let installation_id = session.installation_id().to_string();
    let namespace_id = session.namespace_id;
    let epoch = session.epoch;
    let observed_principal = principal.principal.clone();
    let observed_session = session_id.clone();
    let system = crate::rpc::system(
        stream,
        Side::Server,
        shard.shared.limits.max_message_bytes,
        Some(membrane.bootstrap()),
        move |kind: &str| {
            tracing::error!(
                principal = %observed_principal,
                session_id = %observed_session,
                namespace_id = %namespace_hex(namespace_id),
                kind,
                "a client sent a rejected rpc message; the link is closed"
            );
            let mut content = session_event(
                Event::RpcRejected,
                &SessionFacts {
                    device_id: &device_id,
                    installation_id: &installation_id,
                    namespace_id: Some(namespace_id),
                    epoch: Some(epoch),
                },
                Some(detail(&[("kind", Value::String(kind.to_string()))])),
            );
            content.principal = observed_principal.clone();
            content.session_id = Some(observed_session.clone());
            audit.record_event(content);
        },
    );
    let revocation = membrane.revocation();
    tracing::info!(
        principal = %principal.principal,
        session_id = %session_id,
        namespace_id = %namespace_hex(namespace_id),
        epoch,
        %remote,
        "inner client connected"
    );
    let disconnector = system.get_disconnector();
    let mut system = std::pin::pin!(system);
    let outcome = tokio::select! {
        outcome = &mut system => match outcome {
            Ok(()) => "the client closed the link".to_string(),
            Err(error) => format!("the link failed: {error}"),
        },
        () = revocation.cancelled() => {
            let closing = futures::future::join(disconnector, &mut system);
            if tokio::time::timeout(DISCONNECT_GRACE, closing).await.is_err() {
                tracing::debug!(session_id = %session_id, "a dropped client did not disconnect in time");
            }
            "the membrane was dropped".to_string()
        }
    };
    session.remove_client(&session_id);
    drop(membrane);
    tracing::info!(
        principal = %principal.principal,
        session_id = %session_id,
        namespace_id = %namespace_hex(namespace_id),
        epoch,
        %remote,
        outcome = %outcome,
        "inner client disconnected"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_on_the_not_connected_bootstrap_fails_naming_the_node() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let client: dusk_capnp::dusk_capnp::dusk::Client =
                FromClientHook::new(not_connected_bootstrap(0x00ab).hook);
            let error = client.ps_request().send().promise.await.err().unwrap();
            assert_eq!(error.kind, capnp::ErrorKind::Disconnected);
            assert!(
                error
                    .extra
                    .contains("node 00000000000000ab is not connected"),
                "{error}"
            );
        });
    }
}
