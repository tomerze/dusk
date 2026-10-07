use crate::audit::ParamField;
use crate::schema::{Bundle, FieldKind, FieldSchema, FieldType, StructSchema};
use capnp::any_pointer;
use capnp::message::{Builder, HeapAllocator};
use capnp::private::layout::{
    ElementSize, PointerBuilder, PointerReader, PointerType, StructBuilder, StructReader,
    StructSize,
};
use capnp::traits::{FromPointerBuilder, FromPointerReader, HasTypeId};
use dusk_capnp::dusk_capnp::program_args;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

const MAXIMUM_DEPTH: u32 = 128;
const BITS_PER_WORD: u32 = 64;

pub(crate) struct RawPointerBuilder<'a>(pub(crate) PointerBuilder<'a>);

impl<'a> FromPointerBuilder<'a> for RawPointerBuilder<'a> {
    fn init_pointer(builder: PointerBuilder<'a>, _length: u32) -> RawPointerBuilder<'a> {
        RawPointerBuilder(builder)
    }

    fn get_from_pointer(
        builder: PointerBuilder<'a>,
        _default: Option<&'a [capnp::Word]>,
    ) -> capnp::Result<RawPointerBuilder<'a>> {
        Ok(RawPointerBuilder(builder))
    }
}

pub(crate) struct RawPointerReader<'a>(pub(crate) PointerReader<'a>);

impl<'a> FromPointerReader<'a> for RawPointerReader<'a> {
    fn get_from_pointer(
        reader: &PointerReader<'a>,
        _default: Option<&'a [capnp::Word]>,
    ) -> capnp::Result<RawPointerReader<'a>> {
        Ok(RawPointerReader(*reader))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sanitized {
    pub canonical: Vec<u8>,
    pub fields: Vec<ParamField>,
    pub interfaces: HashMap<usize, u64>,
}

pub(crate) fn program_args_id() -> u64 {
    <program_args::Reader<'static, any_pointer::Owned, any_pointer::Owned> as HasTypeId>::TYPE_ID
}

pub(crate) fn copy_data_section(source: &StructReader, destination: &mut StructBuilder) {
    let source_bits = source.get_data_section_size();
    let destination_bits = destination.as_reader().get_data_section_size();
    let bits = source_bits.min(destination_bits);
    if bits == 1 {
        destination.set_bool_field(0, source.get_bool_field(0));
        return;
    }
    let words = bits / BITS_PER_WORD;
    for word in 0..words {
        destination
            .set_data_field::<u64>(word as usize, source.get_data_field::<u64>(word as usize));
    }
    for byte in (words * 8)..(bits / 8) {
        destination.set_data_field::<u8>(byte as usize, source.get_data_field::<u8>(byte as usize));
    }
}

pub(crate) fn struct_size(reader: &StructReader) -> StructSize {
    StructSize {
        data: reader.get_data_section_size().div_ceil(BITS_PER_WORD) as u16,
        pointers: reader.get_pointer_section_size(),
    }
}

fn bit_range(offset: u32, field_type: &FieldType) -> Option<(u64, u64)> {
    let bits = u64::from(field_type.data_bits()?);
    let start = u64::from(offset) * bits;
    Some((start, start + bits))
}

fn clear_data_slot(destination: &mut StructBuilder, offset: u32, field_type: &FieldType) {
    let Some((_, end)) = bit_range(offset, field_type) else {
        return;
    };
    if end > u64::from(destination.as_reader().get_data_section_size()) {
        return;
    }
    let offset = offset as usize;
    match field_type.data_bits() {
        Some(1) => destination.set_bool_field(offset, false),
        Some(8) => destination.set_data_field::<u8>(offset, 0),
        Some(16) => destination.set_data_field::<u16>(offset, 0),
        Some(32) => destination.set_data_field::<u32>(offset, 0),
        Some(_) => destination.set_data_field::<u64>(offset, 0),
        None => {}
    }
}

#[derive(Default)]
struct Layout<'s> {
    active: Vec<&'s FieldSchema>,
    sensitive_pointers: Vec<u32>,
    sensitive_data: Vec<(u32, FieldType)>,
    active_data: Vec<(u64, u64)>,
}

struct Walk<'a> {
    bundle: &'a Bundle,
    interfaces: HashMap<usize, u64>,
}

impl<'a> Walk<'a> {
    fn discriminant(schema: &StructSchema, source: &StructReader) -> u16 {
        if schema.discriminant_count == 0 {
            return 0;
        }
        let offset = u64::from(schema.discriminant_offset);
        if (offset + 1) * 16 > u64::from(source.get_data_section_size()) {
            return 0;
        }
        source.get_data_field::<u16>(schema.discriminant_offset as usize)
    }

    fn layout(
        &self,
        schema: &'a StructSchema,
        source: &StructReader,
        layout: &mut Layout<'a>,
        active: bool,
    ) {
        let discriminant = Self::discriminant(schema, source);
        for field in &schema.fields {
            let field_active =
                active && field.discriminant.is_none_or(|value| value == discriminant);
            if field_active {
                layout.active.push(field);
            }
            match &field.kind {
                FieldKind::Group { struct_id } => {
                    if let Some(group) = self.bundle.structure(*struct_id) {
                        if field.sensitive {
                            self.mark_sensitive(group, layout);
                        }
                        let mut nested = Layout::default();
                        self.layout(group, source, &mut nested, field_active && !field.sensitive);
                        layout.sensitive_pointers.extend(nested.sensitive_pointers);
                        layout.sensitive_data.extend(nested.sensitive_data);
                        layout.active_data.extend(nested.active_data);
                    }
                }
                FieldKind::Slot { offset, field_type } => {
                    if field.sensitive {
                        if field_type.is_pointer() {
                            layout.sensitive_pointers.push(*offset);
                        } else if field_type.data_bits().is_some() {
                            layout.sensitive_data.push((*offset, field_type.clone()));
                        }
                    } else if field_active && let Some(range) = bit_range(*offset, field_type) {
                        layout.active_data.push(range);
                    }
                }
            }
        }
        if active && schema.discriminant_count > 0 {
            let start = u64::from(schema.discriminant_offset) * 16;
            layout.active_data.push((start, start + 16));
        }
    }

    fn mark_sensitive(&self, schema: &StructSchema, layout: &mut Layout) {
        for field in &schema.fields {
            match &field.kind {
                FieldKind::Group { struct_id } => {
                    if let Some(group) = self.bundle.structure(*struct_id) {
                        self.mark_sensitive(group, layout);
                    }
                }
                FieldKind::Slot { offset, field_type } => {
                    if field_type.is_pointer() {
                        layout.sensitive_pointers.push(*offset);
                    } else if field_type.data_bits().is_some() {
                        layout.sensitive_data.push((*offset, field_type.clone()));
                    }
                }
            }
        }
        if schema.discriminant_count > 0 {
            layout
                .sensitive_data
                .push((schema.discriminant_offset, FieldType::UInt16));
        }
    }

    fn copy_struct(
        &mut self,
        schema: Option<&'a StructSchema>,
        source: StructReader,
        mut destination: StructBuilder,
        depth: u32,
        top: Option<&mut Vec<ParamField>>,
    ) -> bool {
        copy_data_section(&source, &mut destination);
        let mut handled = vec![false; usize::from(source.get_pointer_section_size())];
        let mut redacted = false;
        if let Some(schema) = schema {
            let mut layout = Layout::default();
            self.layout(schema, &source, &mut layout, true);
            let program_data =
                if schema.id == program_args_id() && source.get_data_section_size() >= 64 {
                    self.bundle.program_data(source.get_data_field::<u64>(0))
                } else {
                    None
                };
            let mut fields = Vec::new();
            for field in &layout.active {
                let field_redacted = self.copy_field(
                    field,
                    &source,
                    &mut destination,
                    &mut handled,
                    depth,
                    program_data,
                );
                redacted |= field_redacted;
                fields.push(ParamField {
                    name: field.name.clone(),
                    redacted: field_redacted,
                });
            }
            for (offset, field_type) in &layout.sensitive_data {
                let overlaps = bit_range(*offset, field_type).is_some_and(|(start, end)| {
                    layout.active_data.iter().any(|(active_start, active_end)| {
                        start < *active_end && *active_start < end
                    })
                });
                if !overlaps {
                    clear_data_slot(&mut destination, *offset, field_type);
                }
            }
            for offset in &layout.sensitive_pointers {
                if let Some(slot) = handled.get_mut(*offset as usize)
                    && !*slot
                {
                    *slot = true;
                    if !source.get_pointer_field(*offset as usize).is_null() {
                        redacted = true;
                    }
                }
            }
            if let Some(top) = top {
                *top = fields;
            }
        }
        for (index, done) in handled.iter().enumerate() {
            if !*done {
                redacted |= self.copy_opaque(
                    source.get_pointer_field(index),
                    destination.reborrow().get_pointer_field(index),
                );
            }
        }
        redacted
    }

    fn copy_field(
        &mut self,
        field: &'a FieldSchema,
        source: &StructReader,
        destination: &mut StructBuilder,
        handled: &mut [bool],
        depth: u32,
        program_data: Option<u64>,
    ) -> bool {
        match &field.kind {
            FieldKind::Group { struct_id } => {
                let Some(group) = self.bundle.structure(*struct_id) else {
                    return false;
                };
                if field.sensitive {
                    return true;
                }
                let mut layout = Layout::default();
                self.layout(group, source, &mut layout, true);
                let mut redacted = false;
                for nested in &layout.active {
                    redacted |=
                        self.copy_field(nested, source, destination, handled, depth, program_data);
                }
                redacted
            }
            FieldKind::Slot { offset, field_type } => {
                if !field_type.is_pointer() {
                    return field.sensitive;
                }
                let index = *offset as usize;
                let Some(slot) = handled.get_mut(index) else {
                    return false;
                };
                *slot = true;
                let pointer = source.get_pointer_field(index);
                if field.sensitive {
                    return !pointer.is_null();
                }
                let typed = match (field_type, program_data) {
                    (FieldType::AnyPointer, Some(data)) if field.name == "data" => {
                        FieldType::Struct(data)
                    }
                    _ => field_type.clone(),
                };
                self.copy_pointer(
                    &typed,
                    pointer,
                    destination.reborrow().get_pointer_field(index),
                    depth + 1,
                )
            }
        }
    }

    fn record_capability(&mut self, source: &PointerReader, interface_id: u64) {
        if let Ok(hook) = source.get_capability() {
            let entry = self
                .interfaces
                .entry(hook.get_ptr())
                .or_insert(interface_id);
            if *entry == 0 {
                *entry = interface_id;
            }
        }
    }

    fn copy_pointer(
        &mut self,
        field_type: &FieldType,
        source: PointerReader,
        destination: PointerBuilder,
        depth: u32,
    ) -> bool {
        if source.is_null() {
            return false;
        }
        if depth > MAXIMUM_DEPTH {
            return true;
        }
        let Ok(pointer_type) = source.get_pointer_type() else {
            return true;
        };
        match (field_type, pointer_type) {
            (FieldType::Interface(interface_id), PointerType::Capability) => {
                self.record_capability(&source, *interface_id);
                false
            }
            (FieldType::Interface(_), _) => true,
            (FieldType::Struct(struct_id), PointerType::Struct) => match source.get_struct(None) {
                Ok(reader) => {
                    let size = struct_size(&reader);
                    let schema = self.bundle.structure(*struct_id);
                    let builder = destination.init_struct(size);
                    self.copy_struct(schema, reader, builder, depth, None)
                }
                Err(_) => true,
            },
            (FieldType::List(element), PointerType::List) => {
                self.copy_list(element, source, destination, depth)
            }
            _ => self.copy_opaque(source, destination),
        }
    }

    fn copy_list(
        &mut self,
        element: &FieldType,
        source: PointerReader,
        destination: PointerBuilder,
        depth: u32,
    ) -> bool {
        match element {
            FieldType::Struct(struct_id) => {
                let Ok(list) = source.get_list(ElementSize::InlineComposite, None) else {
                    return true;
                };
                let length = list.len();
                let size = if length == 0 {
                    StructSize {
                        data: 0,
                        pointers: 0,
                    }
                } else {
                    struct_size(&list.get_struct_element(0))
                };
                let schema = self.bundle.structure(*struct_id);
                let mut builder = destination.init_struct_list(length, size);
                let mut redacted = false;
                for index in 0..length {
                    redacted |= self.copy_struct(
                        schema,
                        list.get_struct_element(index),
                        builder.reborrow().get_struct_element(index),
                        depth + 1,
                        None,
                    );
                }
                redacted
            }
            FieldType::Interface(_) | FieldType::List(_) | FieldType::AnyPointer => {
                let Ok(list) = source.get_list(ElementSize::Pointer, None) else {
                    return true;
                };
                let length = list.len();
                let mut builder = destination.init_list(ElementSize::Pointer, length);
                let mut redacted = false;
                for index in 0..length {
                    redacted |= self.copy_pointer(
                        element,
                        list.get_pointer_element(index),
                        builder.reborrow().get_pointer_element(index),
                        depth + 1,
                    );
                }
                redacted
            }
            _ => self.copy_opaque(source, destination),
        }
    }

    fn copy_opaque(&mut self, source: PointerReader, mut destination: PointerBuilder) -> bool {
        match source.get_pointer_type() {
            Ok(PointerType::Null) => false,
            Ok(PointerType::Capability) => {
                self.record_capability(&source, 0);
                false
            }
            Ok(PointerType::Struct | PointerType::List) => {
                if destination.copy_from(source, true).is_err() {
                    destination.clear();
                    true
                } else {
                    false
                }
            }
            Err(_) => true,
        }
    }
}

pub fn sanitize(
    bundle: &Bundle,
    struct_id: Option<u64>,
    source: any_pointer::Reader,
    canonicalize: bool,
) -> capnp::Result<Sanitized> {
    let RawPointerReader(pointer) = source.get_as::<RawPointerReader>()?;
    let mut walk = Walk {
        bundle,
        interfaces: HashMap::new(),
    };
    let mut work = Builder::new_default();
    let mut fields = Vec::new();
    {
        let root: any_pointer::Builder = work.init_root();
        let RawPointerBuilder(destination) = root.get_as::<RawPointerBuilder>()?;
        let schema = struct_id.and_then(|id| bundle.structure(id));
        match (schema, pointer.get_pointer_type()) {
            (Some(schema), Ok(PointerType::Struct)) => match pointer.get_struct(None) {
                Ok(reader) => {
                    let size = struct_size(&reader);
                    let builder = destination.init_struct(size);
                    walk.copy_struct(Some(schema), reader, builder, 1, Some(&mut fields));
                }
                Err(_) => fields.push(ParamField {
                    name: schema.name.clone(),
                    redacted: true,
                }),
            },
            (Some(schema), Ok(PointerType::Null)) => {
                for field in &schema.fields {
                    if field.discriminant.is_none_or(|value| value == 0) {
                        fields.push(ParamField {
                            name: field.name.clone(),
                            redacted: false,
                        });
                    }
                }
            }
            _ => {
                walk.copy_opaque(pointer, destination);
            }
        }
    }
    let canonical = if canonicalize {
        let reader: any_pointer::Reader = work.get_root_as_reader()?;
        let words = reader.target_size()?.word_count.saturating_add(1);
        let mut canonical = Builder::new(
            HeapAllocator::new().first_segment_words(u32::try_from(words).unwrap_or(u32::MAX)),
        );
        canonical.set_root_canonical::<any_pointer::Owned>(reader)?;
        let segments = canonical.get_segments_for_output();
        segments
            .iter()
            .flat_map(|segment| segment.iter().copied())
            .collect()
    } else {
        Vec::new()
    };
    Ok(Sanitized {
        canonical,
        fields,
        interfaces: walk.interfaces,
    })
}

pub fn param_hash(key: &[u8], canonical: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 takes a key of any length");
    mac.update(canonical);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaRegistry;
    use capnp::private::layout::CapTable;
    use capnp::traits::{Imbue, ImbueMut};
    use dusk_base::dusk_program_kvs::kvs_capnp::{kvs_args, kvs_portal};
    use dusk_capnp::dusk_capnp::{dusk, process, stream};
    use std::path::Path;
    use std::sync::Arc;

    const KEY: &[u8] = b"canonical test param key";

    struct Inert;

    impl process::Server for Inert {}

    impl stream::Server for Inert {}

    fn bundle() -> Arc<Bundle> {
        SchemaRegistry::load_directory(Path::new(env!("NIGHTFALL_TREE_SCHEMAS")))
            .unwrap()
            .bundle_for(&[])
    }

    struct Message {
        builder: Builder<HeapAllocator>,
        table: CapTable,
    }

    fn build(fill: impl FnOnce(any_pointer::Builder)) -> Message {
        let mut builder = Builder::new_default();
        let mut table = CapTable::new();
        {
            let mut root: any_pointer::Builder = builder.init_root();
            root.imbue_mut(&mut table);
            fill(root);
        }
        Message { builder, table }
    }

    fn sanitized(bundle: &Bundle, struct_id: u64, message: &Message) -> Sanitized {
        let mut reader: any_pointer::Reader = message.builder.get_root_as_reader().unwrap();
        reader.imbue(&message.table);
        sanitize(bundle, Some(struct_id), reader, true).unwrap()
    }

    fn process_client() -> process::Client {
        capnp_rpc::new_client(Inert)
    }

    fn method<Client: HasTypeId>(bundle: &Bundle, ordinal: u16, action: &str) -> (u64, u64) {
        let resolved = bundle.method(Client::TYPE_ID, ordinal).unwrap();
        assert_eq!(resolved.action(), action);
        (resolved.method.param_struct, resolved.method.result_struct)
    }

    fn kvs_set(key: u64, value: &str) -> Message {
        build(|root| {
            let mut params = root.init_as::<kvs_portal::set_params::Builder>();
            params.set_key(key);
            params.init_value().set_string(value);
        })
    }

    #[test]
    fn hashes_the_same_whatever_a_sensitive_parameter_holds() {
        let bundle = bundle();
        let (set, _) = method::<kvs_portal::Client>(&bundle, 1, "KvsPortal.set");
        let first = sanitized(&bundle, set, &kvs_set(7, "first secret"));
        let second = sanitized(&bundle, set, &kvs_set(7, "second secret"));
        let other_key = sanitized(&bundle, set, &kvs_set(8, "first secret"));
        assert_eq!(
            param_hash(KEY, &first.canonical),
            param_hash(KEY, &second.canonical)
        );
        assert_ne!(
            param_hash(KEY, &first.canonical),
            param_hash(KEY, &other_key.canonical)
        );
        assert_eq!(
            first.fields,
            [
                ParamField {
                    name: "key".to_string(),
                    redacted: false
                },
                ParamField {
                    name: "value".to_string(),
                    redacted: true
                },
                ParamField {
                    name: "forbiddenUnstick".to_string(),
                    redacted: false
                },
                ParamField {
                    name: "flags".to_string(),
                    redacted: false
                }
            ]
        );
        let needle = b"first secret";
        assert!(
            !first
                .canonical
                .windows(needle.len())
                .any(|window| window == needle)
        );
    }

    #[test]
    fn hashes_the_same_whatever_capability_a_call_carries() {
        let bundle = bundle();
        let (run, _) = method::<dusk::Client>(&bundle, 1, "Dusk.run");
        let first_process = process_client();
        let first_pointer = first_process.client.hook.get_ptr();
        let first = build(|root| {
            root.init_as::<dusk::run_params::Builder>()
                .set_process(first_process);
        });
        let second = build(|root| {
            root.init_as::<dusk::run_params::Builder>()
                .set_process(process_client());
        });
        let empty = build(|root| {
            root.init_as::<dusk::run_params::Builder>();
        });
        let first = sanitized(&bundle, run, &first);
        let second = sanitized(&bundle, run, &second);
        let empty = sanitized(&bundle, run, &empty);
        assert_eq!(first.canonical, second.canonical);
        assert_eq!(first.canonical, empty.canonical);
        assert_eq!(
            first.fields,
            [ParamField {
                name: "process".to_string(),
                redacted: false
            }]
        );
        assert_eq!(
            first.interfaces.get(&first_pointer),
            Some(&<process::Client as HasTypeId>::TYPE_ID)
        );
    }

    fn kvs_program(value: &str, created: bool) -> Message {
        build(|root| {
            let mut params = root.init_as::<dusk::process_params::Builder>();
            let mut program_args = params.reborrow().init_program_args();
            program_args.set_program_id(dusk_base::dusk_program_kvs::kvs_capnp::PROGRAM_ID);
            let mut set = program_args
                .reborrow()
                .init_args()
                .get_data()
                .unwrap()
                .init_as::<kvs_args::data::Builder>()
                .init_set();
            set.set_key(3);
            set.init_value().set_string(value);
            if created {
                let stream: stream::Client = capnp_rpc::new_client(Inert);
                program_args
                    .get_args()
                    .get_server()
                    .unwrap()
                    .set_as_capability(stream.client.hook);
            }
        })
    }

    #[test]
    fn walks_program_arguments_with_the_programs_own_schema() {
        let bundle = bundle();
        let (process, _) = method::<dusk::Client>(&bundle, 0, "Dusk.process");
        let first = sanitized(&bundle, process, &kvs_program("one", false));
        let second = sanitized(&bundle, process, &kvs_program("two", true));
        assert_eq!(first.canonical, second.canonical);
        assert_eq!(
            first.fields,
            [ParamField {
                name: "programArgs".to_string(),
                redacted: true
            }]
        );
        assert_eq!(second.interfaces.len(), 1);
    }

    #[test]
    fn clears_and_flags_opaque_content_that_holds_a_capability() {
        let bundle = bundle();
        let (process, _) = method::<dusk::Client>(&bundle, 0, "Dusk.process");
        let with_capability = |text: &str| {
            build(|root| {
                let params = root.init_as::<dusk::process_params::Builder>();
                let mut program_args = params.init_program_args();
                program_args.set_program_id(0x1234);
                let data = program_args.init_args().get_data().unwrap();
                let RawPointerBuilder(pointer) = data.get_as::<RawPointerBuilder>().unwrap();
                let mut structure = pointer.init_struct(StructSize {
                    data: 0,
                    pointers: 2,
                });
                structure
                    .reborrow()
                    .get_pointer_field(0)
                    .set_text(text.into());
                let stream: stream::Client = capnp_rpc::new_client(Inert);
                structure
                    .get_pointer_field(1)
                    .set_capability(stream.client.hook);
            })
        };
        let first = sanitized(&bundle, process, &with_capability("alpha"));
        let second = sanitized(&bundle, process, &with_capability("beta"));
        assert_eq!(first.canonical, second.canonical);
        assert!(first.fields[0].redacted);
        assert_eq!(first.interfaces.len(), 0);
    }

    #[test]
    fn hashes_opaque_content_without_capabilities() {
        let bundle = bundle();
        let (process, _) = method::<dusk::Client>(&bundle, 0, "Dusk.process");
        let plain = |text: &str| {
            build(|root| {
                let params = root.init_as::<dusk::process_params::Builder>();
                let mut program_args = params.init_program_args();
                program_args.set_program_id(0x1234);
                program_args
                    .init_args()
                    .get_data()
                    .unwrap()
                    .set_as::<capnp::text::Owned>(text)
                    .unwrap();
            })
        };
        let first = sanitized(&bundle, process, &plain("alpha"));
        let second = sanitized(&bundle, process, &plain("beta"));
        assert_ne!(first.canonical, second.canonical);
        assert!(!first.fields[0].redacted);
    }

    #[test]
    fn hashes_parameters_larger_than_one_default_segment() {
        let bundle = bundle();
        let (process, _) = method::<dusk::Client>(&bundle, 0, "Dusk.process");
        let large = |byte: u8| {
            build(|root| {
                let params = root.init_as::<dusk::process_params::Builder>();
                let mut program_args = params.init_program_args();
                program_args.set_program_id(0x1234);
                program_args
                    .init_args()
                    .get_data()
                    .unwrap()
                    .set_as::<capnp::data::Owned>(&vec![byte; 64 * 1024][..])
                    .unwrap();
            })
        };
        let first = sanitized(&bundle, process, &large(1));
        let again = sanitized(&bundle, process, &large(1));
        let second = sanitized(&bundle, process, &large(2));
        assert!(first.canonical.len() > 64 * 1024);
        assert_eq!(first.canonical, again.canonical);
        assert_ne!(first.canonical, second.canonical);
    }

    #[test]
    fn records_every_capability_in_a_list_of_structs() {
        let bundle = bundle();
        let (_, ps) = method::<dusk::Client>(&bundle, 2, "Dusk.ps");
        let processes = [process_client(), process_client()];
        let pointers: Vec<usize> = processes
            .iter()
            .map(|process| process.client.hook.get_ptr())
            .collect();
        let message = build(|root| {
            let mut entries = root
                .init_as::<dusk::ps_results::Builder>()
                .init_process_entries(3);
            for (index, process) in processes.into_iter().enumerate() {
                let mut entry = entries.reborrow().get(index as u32);
                entry.set_pid(index as u64 + 1);
                entry.set_process(process);
            }
            entries.reborrow().get(2).set_pid(3);
        });
        let result = sanitized(&bundle, ps, &message);
        let process_id = <process::Client as HasTypeId>::TYPE_ID;
        for pointer in pointers {
            assert_eq!(result.interfaces.get(&pointer), Some(&process_id));
        }
        assert_eq!(result.interfaces.len(), 2);
    }

    #[test]
    fn lists_the_fields_of_absent_parameters() {
        let bundle = bundle();
        let (kill, _) = method::<dusk::Client>(&bundle, 4, "Dusk.kill");
        let result = sanitized(&bundle, kill, &build(|_| {}));
        let names: Vec<&str> = result
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(names, ["pid", "signal"]);
    }

    #[test]
    fn computes_hmac_sha256() {
        assert_eq!(
            param_hash(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
