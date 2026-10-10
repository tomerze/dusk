use crate::canonical::{RawPointerBuilder, RawPointerReader, copy_data_section, struct_size};
use capnp::any_pointer;
use capnp::message::{Builder, HeapAllocator};
use capnp::private::capability::{ClientHook, PipelineOp};
use capnp::private::layout::{CapTable, PointerType, StructBuilder, StructReader, StructSize};
use capnp::traits::{Imbue, ImbueMut};

pub(crate) struct Scratch {
    message: Builder<HeapAllocator>,
    table: CapTable,
}

impl Scratch {
    pub(crate) fn copy(source: any_pointer::Reader) -> capnp::Result<Scratch> {
        let mut message = Builder::new_default();
        let mut table: CapTable = Vec::new();
        {
            let mut root: any_pointer::Builder = message.init_root();
            root.imbue_mut(&mut table);
            root.set_as(source)?;
        }
        Ok(Scratch { message, table })
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.message.size_in_words() as u64 * 8
    }

    pub(crate) fn reader(&self) -> capnp::Result<any_pointer::Reader<'_>> {
        self.reader_with(&self.table)
    }

    pub(crate) fn reader_with<'a>(
        &'a self,
        table: &'a CapTable,
    ) -> capnp::Result<any_pointer::Reader<'a>> {
        let mut reader: any_pointer::Reader = self.message.get_root_as_reader()?;
        reader.imbue(table);
        Ok(reader)
    }

    pub(crate) fn take_table(&mut self) -> CapTable {
        std::mem::take(&mut self.table)
    }
}

pub(crate) fn path_operations(pointers: &[u16]) -> Vec<PipelineOp> {
    pointers
        .iter()
        .map(|pointer| PipelineOp::GetPointerField(*pointer))
        .collect()
}

pub(crate) fn place_capability(
    root: any_pointer::Builder,
    pointers: &[u16],
    sizes: &[StructSize],
    hook: Box<dyn ClientHook>,
) -> capnp::Result<()> {
    let RawPointerBuilder(pointer) = root.get_as::<RawPointerBuilder>()?;
    let mut structure = pointer.get_struct(sizes[0], None)?;
    let last = pointers.len() - 1;
    for level in 0..last {
        structure = structure
            .get_pointer_field(usize::from(pointers[level]))
            .get_struct(sizes[level + 1], None)?;
    }
    structure
        .get_pointer_field(usize::from(pointers[last]))
        .set_capability(hook);
    Ok(())
}

fn merge_struct(
    source: StructReader,
    destination: StructBuilder,
    paths: &[&[u16]],
) -> capnp::Result<()> {
    let mut destination = destination;
    copy_data_section(&source, &mut destination);
    let source_pointers = source.get_pointer_section_size();
    for index in 0..source_pointers {
        let below: Vec<&[u16]> = paths
            .iter()
            .filter(|path| path.first() == Some(&index))
            .map(|path| &path[1..])
            .collect();
        let source_pointer = source.get_pointer_field(usize::from(index));
        if below.is_empty() {
            destination
                .reborrow()
                .get_pointer_field(usize::from(index))
                .copy_from(source_pointer, false)?;
        } else if below.iter().any(|rest| rest.is_empty()) {
            continue;
        } else {
            if source_pointer.is_null() {
                continue;
            }
            if !matches!(source_pointer.get_pointer_type()?, PointerType::Struct) {
                return Err(capnp::Error::failed(
                    "a result holds a non-struct pointer where its schema has a struct".to_string(),
                ));
            }
            let nested_source = source_pointer.get_struct(None)?;
            let size = struct_size(&nested_source);
            let nested_destination = destination
                .reborrow()
                .get_pointer_field(usize::from(index))
                .get_struct(size, None)?;
            merge_struct(nested_source, nested_destination, &below)?;
        }
    }
    Ok(())
}

pub(crate) fn merge_into(
    root: any_pointer::Builder,
    source: any_pointer::Reader,
    paths: &[&[u16]],
) -> capnp::Result<()> {
    let RawPointerReader(source_pointer) = source.get_as::<RawPointerReader>()?;
    if source_pointer.is_null() {
        return Ok(());
    }
    if !matches!(source_pointer.get_pointer_type()?, PointerType::Struct) {
        return Err(capnp::Error::failed(
            "a result is not a struct where its schema has one".to_string(),
        ));
    }
    let source_struct = source_pointer.get_struct(None)?;
    let size = struct_size(&source_struct);
    let RawPointerBuilder(pointer) = root.get_as::<RawPointerBuilder>()?;
    let destination = pointer.get_struct(size, None)?;
    merge_struct(source_struct, destination, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp::private::layout::PointerReader;
    use dusk_capnp::dusk_capnp::{dusk, process};

    struct Inert;

    impl process::Server for Inert {}

    fn process_client() -> process::Client {
        capnp_rpc::new_client(Inert)
    }

    fn hook_pointer(client: &process::Client) -> usize {
        client.client.hook.get_ptr()
    }

    struct Source {
        message: Builder<HeapAllocator>,
        table: CapTable,
    }

    impl Source {
        fn new(fill: impl FnOnce(any_pointer::Builder)) -> Source {
            let mut message = Builder::new_default();
            let mut table = CapTable::new();
            {
                let mut root: any_pointer::Builder = message.init_root();
                root.imbue_mut(&mut table);
                fill(root);
            }
            Source { message, table }
        }

        fn reader(&self) -> any_pointer::Reader<'_> {
            let mut reader: any_pointer::Reader = self.message.get_root_as_reader().unwrap();
            reader.imbue(&self.table);
            reader
        }
    }

    #[test]
    fn moves_every_capability_of_nested_lists_and_structs_into_its_own_table() {
        let first = process_client();
        let second = process_client();
        let pointers = [hook_pointer(&first), hook_pointer(&second)];
        let source = Source::new(|root| {
            let mut entries = root
                .init_as::<dusk::ps_results::Builder>()
                .init_process_entries(3);
            entries.reborrow().get(0).set_pid(10);
            entries.reborrow().get(0).set_process(first);
            entries.reborrow().get(1).set_pid(11);
            entries.reborrow().get(2).set_pid(12);
            entries.reborrow().get(2).set_process(second);
        });
        let mut scratch = Scratch::copy(source.reader()).unwrap();
        drop(source);
        let copied: dusk::ps_results::Reader = scratch.reader().unwrap().get_as().unwrap();
        let entries = copied.get_process_entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.get(1).get_pid(), 11);
        assert!(entries.get(1).get_process().is_err());
        assert_eq!(
            hook_pointer(&entries.get(2).get_process().unwrap()),
            pointers[1]
        );
        let table = scratch.take_table();
        let table_pointers: Vec<usize> = table
            .iter()
            .map(|hook| hook.as_ref().unwrap().get_ptr())
            .collect();
        assert_eq!(table_pointers, pointers);
    }

    #[test]
    fn reads_the_copy_through_a_substituted_table() {
        let original = process_client();
        let substitute = process_client();
        let substitute_pointer = hook_pointer(&substitute);
        let source = Source::new(|root| {
            root.init_as::<dusk::run_params::Builder>()
                .set_process(original);
        });
        let mut scratch = Scratch::copy(source.reader()).unwrap();
        let replaced = scratch.take_table();
        assert_eq!(replaced.len(), 1);
        let mapped: CapTable = vec![Some(substitute.client.hook)];
        let reader: dusk::run_params::Reader =
            scratch.reader_with(&mapped).unwrap().get_as().unwrap();
        assert_eq!(
            hook_pointer(&reader.get_process().unwrap()),
            substitute_pointer
        );
        let mut outgoing = Builder::new_default();
        let mut outgoing_table = CapTable::new();
        {
            let mut root: any_pointer::Builder = outgoing.init_root();
            root.imbue_mut(&mut outgoing_table);
            root.set_as(scratch.reader_with(&mapped).unwrap()).unwrap();
        }
        assert_eq!(outgoing_table.len(), 1);
        assert_eq!(
            outgoing_table[0].as_ref().unwrap().get_ptr(),
            substitute_pointer
        );
        assert!(scratch.bytes() > 0);
    }

    #[test]
    fn copies_a_message_without_capabilities_or_a_root() {
        let empty = Source::new(|_| {});
        let mut scratch = Scratch::copy(empty.reader()).unwrap();
        assert!(scratch.reader().unwrap().is_null());
        assert!(scratch.take_table().is_empty());
    }

    fn nested(root: any_pointer::Builder, capability: Option<process::Client>, text: &str) {
        let RawPointerBuilder(pointer) = root.get_as::<RawPointerBuilder>().unwrap();
        let mut outer = pointer.init_struct(StructSize {
            data: 1,
            pointers: 2,
        });
        outer.set_data_field::<u64>(0, 77);
        let mut inner = outer
            .reborrow()
            .get_pointer_field(0)
            .init_struct(StructSize {
                data: 0,
                pointers: 2,
            });
        if let Some(capability) = capability {
            inner
                .reborrow()
                .get_pointer_field(0)
                .set_capability(capability.client.hook);
        }
        inner.get_pointer_field(1).set_text(text.into());
        outer.get_pointer_field(1).set_text("outer".into());
    }

    fn text_at(reader: PointerReader) -> String {
        reader.get_text(None).unwrap().to_string().unwrap()
    }

    #[test]
    fn places_a_pipelined_capability_and_merges_the_response_around_it() {
        let placed = process_client();
        let placed_pointer = hook_pointer(&placed);
        let response = Source::new(|root| nested(root, Some(process_client()), "inner"));
        let mut results = Builder::new_default();
        let mut results_table = CapTable::new();
        {
            let mut root: any_pointer::Builder = results.init_root();
            root.imbue_mut(&mut results_table);
            let sizes = [
                StructSize {
                    data: 1,
                    pointers: 2,
                },
                StructSize {
                    data: 0,
                    pointers: 2,
                },
            ];
            place_capability(root.reborrow(), &[0, 0], &sizes, placed.client.hook).unwrap();
            merge_into(root, response.reader(), &[&[0, 0]]).unwrap();
        }
        assert_eq!(results_table.len(), 1);
        let mut reader: any_pointer::Reader = results.get_root_as_reader().unwrap();
        reader.imbue(&results_table);
        let RawPointerReader(root) = reader.get_as::<RawPointerReader>().unwrap();
        let outer = root.get_struct(None).unwrap();
        assert_eq!(outer.get_data_field::<u64>(0), 77);
        assert_eq!(text_at(outer.get_pointer_field(1)), "outer");
        let inner = outer.get_pointer_field(0).get_struct(None).unwrap();
        assert_eq!(text_at(inner.get_pointer_field(1)), "inner");
        assert_eq!(
            inner
                .get_pointer_field(0)
                .get_capability()
                .unwrap()
                .get_ptr(),
            placed_pointer
        );
        let pipelined = reader.get_pipelined_cap(&path_operations(&[0, 0])).unwrap();
        assert_eq!(pipelined.get_ptr(), placed_pointer);
    }

    #[test]
    fn merges_a_response_whose_path_is_null() {
        let response = Source::new(|root| nested(root, None, "inner"));
        let mut results = Builder::new_default();
        let mut results_table = CapTable::new();
        {
            let mut root: any_pointer::Builder = results.init_root();
            root.imbue_mut(&mut results_table);
            merge_into(root, response.reader(), &[&[1]]).unwrap();
        }
        let reader: any_pointer::Reader = results.get_root_as_reader().unwrap();
        let RawPointerReader(root) = reader.get_as::<RawPointerReader>().unwrap();
        let outer = root.get_struct(None).unwrap();
        assert!(outer.get_pointer_field(1).is_null());
        let inner = outer.get_pointer_field(0).get_struct(None).unwrap();
        assert_eq!(text_at(inner.get_pointer_field(1)), "inner");
        assert!(results_table.is_empty());
    }

    #[test]
    fn refuses_a_response_with_a_list_where_its_schema_has_a_struct() {
        let response = Source::new(|root| {
            root.init_as::<dusk::ps_results::Builder>()
                .init_process_entries(1);
        });
        let mut results = Builder::new_default();
        let mut results_table = CapTable::new();
        let mut root: any_pointer::Builder = results.init_root();
        root.imbue_mut(&mut results_table);
        assert!(merge_into(root, response.reader(), &[&[0, 0]]).is_err());
    }
}
