use anyhow::{Context, bail};
use capnp::schema_capnp::{code_generator_request, field, node, type_};
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

pub const STREAM_RESULT_ID: u64 = 0x995f_9a33_77c0_b16e;
pub const SENSITIVE_ANNOTATION_ID: u64 = 0xfd6e_e50f_2bba_77d2;
const MAXIMUM_PIPELINE_DEPTH: usize = 8;
const MAXIMUM_BUNDLE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramVersion {
    pub program_id: u64,
    pub version: String,
    pub git_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    Void,
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Text,
    Data,
    List(Box<FieldType>),
    Enum,
    Struct(u64),
    Interface(u64),
    AnyPointer,
}

impl FieldType {
    fn read(reader: type_::Reader) -> capnp::Result<FieldType> {
        Ok(match reader.which()? {
            type_::Void(()) => FieldType::Void,
            type_::Bool(()) => FieldType::Bool,
            type_::Int8(()) => FieldType::Int8,
            type_::Int16(()) => FieldType::Int16,
            type_::Int32(()) => FieldType::Int32,
            type_::Int64(()) => FieldType::Int64,
            type_::Uint8(()) => FieldType::UInt8,
            type_::Uint16(()) => FieldType::UInt16,
            type_::Uint32(()) => FieldType::UInt32,
            type_::Uint64(()) => FieldType::UInt64,
            type_::Float32(()) => FieldType::Float32,
            type_::Float64(()) => FieldType::Float64,
            type_::Text(()) => FieldType::Text,
            type_::Data(()) => FieldType::Data,
            type_::List(list) => {
                FieldType::List(Box::new(FieldType::read(list.get_element_type()?)?))
            }
            type_::Enum(_) => FieldType::Enum,
            type_::Struct(structure) => FieldType::Struct(structure.get_type_id()),
            type_::Interface(interface) => FieldType::Interface(interface.get_type_id()),
            type_::AnyPointer(_) => FieldType::AnyPointer,
        })
    }

    pub fn data_bits(&self) -> Option<u32> {
        match self {
            FieldType::Bool => Some(1),
            FieldType::Int8 | FieldType::UInt8 => Some(8),
            FieldType::Int16 | FieldType::UInt16 | FieldType::Enum => Some(16),
            FieldType::Int32 | FieldType::UInt32 | FieldType::Float32 => Some(32),
            FieldType::Int64 | FieldType::UInt64 | FieldType::Float64 => Some(64),
            _ => None,
        }
    }

    pub fn is_pointer(&self) -> bool {
        matches!(
            self,
            FieldType::Text
                | FieldType::Data
                | FieldType::List(_)
                | FieldType::Struct(_)
                | FieldType::Interface(_)
                | FieldType::AnyPointer
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Slot { offset: u32, field_type: FieldType },
    Group { struct_id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub discriminant: Option<u16>,
    pub sensitive: bool,
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructSchema {
    pub id: u64,
    pub name: String,
    pub data_words: u16,
    pub pointers: u16,
    pub is_group: bool,
    pub discriminant_count: u16,
    pub discriminant_offset: u32,
    pub fields: Vec<FieldSchema>,
}

impl StructSchema {
    pub fn size(&self) -> capnp::private::layout::StructSize {
        capnp::private::layout::StructSize {
            data: self.data_words,
            pointers: self.pointers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelinePath {
    pub pointers: Vec<u16>,
    pub structs: Vec<u64>,
    pub interface_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodSchema {
    pub name: String,
    pub ordinal: u16,
    pub param_struct: u64,
    pub result_struct: u64,
    pub streaming: bool,
    pub pipeline_paths: Vec<PipelinePath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceSchema {
    pub id: u64,
    pub name: String,
    pub methods: Vec<MethodSchema>,
    pub superclasses: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ManifestProgram {
    pub program_id: String,
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct BundleManifest {
    pub version: String,
    pub git_revision: String,
    pub programs: Vec<ManifestProgram>,
}

pub struct ResolvedMethod<'a> {
    pub interface: &'a InterfaceSchema,
    pub method: &'a MethodSchema,
}

impl ResolvedMethod<'_> {
    pub fn action(&self) -> String {
        format!("{}.{}", self.interface.name, self.method.name)
    }
}

#[derive(Debug)]
pub struct Bundle {
    name: String,
    manifest: BundleManifest,
    program_versions: HashMap<u64, String>,
    interfaces: HashMap<u64, InterfaceSchema>,
    structs: HashMap<u64, StructSchema>,
    program_data: HashMap<u64, u64>,
}

fn short_name(display_name: &str) -> &str {
    match display_name.find(':') {
        Some(index) => &display_name[index + 1..],
        None => display_name,
    }
}

fn read_struct(
    id: u64,
    name: String,
    structure: node::struct_::Reader,
) -> capnp::Result<StructSchema> {
    let mut fields = Vec::new();
    for field_reader in structure.get_fields()? {
        let discriminant = match field_reader.get_discriminant_value() {
            field::NO_DISCRIMINANT => None,
            value => Some(value),
        };
        let sensitive = field_reader
            .get_annotations()?
            .iter()
            .any(|annotation| annotation.get_id() == SENSITIVE_ANNOTATION_ID);
        let kind = match field_reader.which()? {
            field::Slot(slot) => FieldKind::Slot {
                offset: slot.get_offset(),
                field_type: FieldType::read(slot.get_type()?)?,
            },
            field::Group(group) => FieldKind::Group {
                struct_id: group.get_type_id(),
            },
        };
        fields.push(FieldSchema {
            name: field_reader.get_name()?.to_string()?,
            discriminant,
            sensitive,
            kind,
        });
    }
    Ok(StructSchema {
        id,
        name,
        data_words: structure.get_data_word_count(),
        pointers: structure.get_pointer_count(),
        is_group: structure.get_is_group(),
        discriminant_count: structure.get_discriminant_count(),
        discriminant_offset: structure.get_discriminant_offset(),
        fields,
    })
}

fn same_revision(bundle: &str, program: &str) -> bool {
    !bundle.is_empty()
        && !program.is_empty()
        && (bundle.starts_with(program) || program.starts_with(bundle))
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let components = |version: &str| -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .map(|component| component.parse::<u64>().unwrap_or(0))
            .collect()
    };
    components(left).cmp(&components(right))
}

#[derive(Default)]
struct BundleBuilder {
    interfaces: HashMap<u64, InterfaceSchema>,
    structs: HashMap<u64, StructSchema>,
    program_constants: Vec<(u64, u64)>,
    args_data: HashMap<u64, Vec<u64>>,
    scopes: HashMap<u64, u64>,
}

impl BundleBuilder {
    fn add_node(&mut self, node_reader: node::Reader) -> capnp::Result<()> {
        let id = node_reader.get_id();
        let name = short_name(node_reader.get_display_name()?.to_str()?).to_string();
        self.scopes.insert(id, node_reader.get_scope_id());
        match node_reader.which()? {
            node::Struct(structure) => {
                if name.ends_with("Args.Data") && name.matches('.').count() == 1 {
                    let args_scope = node_reader.get_scope_id();
                    self.args_data.entry(args_scope).or_default().push(id);
                }
                self.structs.insert(id, read_struct(id, name, structure)?);
            }
            node::Interface(interface) => {
                let mut methods = Vec::new();
                for (ordinal, method) in interface.get_methods()?.iter().enumerate() {
                    methods.push(MethodSchema {
                        name: method.get_name()?.to_string()?,
                        ordinal: ordinal as u16,
                        param_struct: method.get_param_struct_type(),
                        result_struct: method.get_result_struct_type(),
                        streaming: method.get_result_struct_type() == STREAM_RESULT_ID,
                        pipeline_paths: Vec::new(),
                    });
                }
                let superclasses = interface
                    .get_superclasses()?
                    .iter()
                    .map(|superclass| superclass.get_id())
                    .collect();
                self.interfaces.insert(
                    id,
                    InterfaceSchema {
                        id,
                        name,
                        methods,
                        superclasses,
                    },
                );
            }
            node::Const(constant) => {
                if name == "programId"
                    && let capnp::schema_capnp::value::Uint64(program_id) =
                        constant.get_value()?.which()?
                {
                    self.program_constants
                        .push((program_id, node_reader.get_scope_id()));
                }
            }
            node::File(()) | node::Enum(_) | node::Annotation(_) => {}
        }
        Ok(())
    }

    fn pointer_slots(&self, struct_id: u64, slots: &mut HashMap<u32, Vec<FieldType>>) {
        let Some(structure) = self.structs.get(&struct_id) else {
            return;
        };
        for field_schema in &structure.fields {
            match &field_schema.kind {
                FieldKind::Group { struct_id: group } => self.pointer_slots(*group, slots),
                FieldKind::Slot { offset, field_type } if field_type.is_pointer() => {
                    slots.entry(*offset).or_default().push(field_type.clone());
                }
                FieldKind::Slot { .. } => {}
            }
        }
    }

    fn collect_paths(
        &self,
        physical: u64,
        prefix: &mut (Vec<u16>, Vec<u64>),
        visiting: &mut HashSet<u64>,
        paths: &mut Vec<PipelinePath>,
    ) {
        if prefix.0.len() >= MAXIMUM_PIPELINE_DEPTH
            || !self.structs.contains_key(&physical)
            || !visiting.insert(physical)
        {
            return;
        }
        let mut slots = HashMap::new();
        self.pointer_slots(physical, &mut slots);
        let mut offsets: Vec<u32> = slots.keys().copied().collect();
        offsets.sort_unstable();
        for offset in offsets {
            let Ok(pointer) = u16::try_from(offset) else {
                continue;
            };
            let types = &slots[&offset];
            if types
                .iter()
                .all(|field_type| matches!(field_type, FieldType::Interface(_)))
            {
                let interface_id = match types[0] {
                    FieldType::Interface(first)
                        if types.iter().all(|field_type| *field_type == types[0]) =>
                    {
                        first
                    }
                    _ => 0,
                };
                let mut pointers = prefix.0.clone();
                pointers.push(pointer);
                let mut structs = prefix.1.clone();
                structs.push(physical);
                paths.push(PipelinePath {
                    pointers,
                    structs,
                    interface_id,
                });
            } else if let FieldType::Struct(child) = types[0]
                && types.iter().all(|field_type| *field_type == types[0])
            {
                prefix.0.push(pointer);
                prefix.1.push(physical);
                self.collect_paths(child, prefix, visiting, paths);
                prefix.0.pop();
                prefix.1.pop();
            }
        }
        visiting.remove(&physical);
    }

    fn finish(mut self, name: String, manifest: BundleManifest) -> Bundle {
        let mut all_paths = HashMap::new();
        for interface in self.interfaces.values() {
            for method in &interface.methods {
                if method.streaming || all_paths.contains_key(&method.result_struct) {
                    continue;
                }
                let mut paths = Vec::new();
                self.collect_paths(
                    method.result_struct,
                    &mut (Vec::new(), Vec::new()),
                    &mut HashSet::new(),
                    &mut paths,
                );
                all_paths.insert(method.result_struct, paths);
            }
        }
        for interface in self.interfaces.values_mut() {
            for method in &mut interface.methods {
                if let Some(paths) = all_paths.get(&method.result_struct) {
                    method.pipeline_paths = paths.clone();
                }
            }
        }
        let mut program_data = HashMap::new();
        for (program_id, file) in &self.program_constants {
            let candidates: Vec<u64> = self
                .args_data
                .iter()
                .filter(|(args, _)| self.scopes.get(args) == Some(file))
                .flat_map(|(_, data)| data.iter().copied())
                .collect();
            match candidates.as_slice() {
                [data] => {
                    program_data.insert(*program_id, *data);
                }
                [] => {}
                _ => tracing::warn!(
                    bundle = %name,
                    program_id = format!("{program_id:016x}"),
                    "a program's schema file declares more than one Args.Data struct; its args data is hashed without its schema"
                ),
            }
        }
        let program_versions = manifest
            .programs
            .iter()
            .filter_map(|program| {
                u64::from_str_radix(&program.program_id, 16)
                    .ok()
                    .map(|id| (id, program.version.clone()))
            })
            .chain(
                self.program_constants
                    .iter()
                    .map(|(program_id, _)| (*program_id, String::new())),
            )
            .fold(HashMap::new(), |mut versions, (id, version)| {
                versions.entry(id).or_insert(version);
                versions
            });
        Bundle {
            name,
            manifest,
            program_versions,
            interfaces: self.interfaces,
            structs: self.structs,
            program_data,
        }
    }
}

impl Bundle {
    pub fn parse(name: &str, bytes: &[u8], manifest: BundleManifest) -> anyhow::Result<Bundle> {
        if bytes.len() > MAXIMUM_BUNDLE_BYTES {
            bail!(
                "schema bundle {name} is {} bytes, more than {MAXIMUM_BUNDLE_BYTES}",
                bytes.len()
            );
        }
        let options = capnp::message::ReaderOptions {
            traversal_limit_in_words: Some(MAXIMUM_BUNDLE_BYTES / 8),
            nesting_limit: 64,
        };
        let message = capnp::serialize::read_message_from_flat_slice(&mut &bytes[..], options)
            .with_context(|| format!("schema bundle {name} is not a Cap'n Proto message"))?;
        let request: code_generator_request::Reader = message
            .get_root()
            .with_context(|| format!("schema bundle {name} is not a CodeGeneratorRequest"))?;
        let mut builder = BundleBuilder::default();
        for node_reader in request.get_nodes()? {
            builder
                .add_node(node_reader)
                .with_context(|| format!("schema bundle {name} holds an unreadable node"))?;
        }
        if builder.interfaces.is_empty() {
            bail!("schema bundle {name} holds no interfaces");
        }
        Ok(builder.finish(name.to_string(), manifest))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }

    pub fn interface(&self, interface_id: u64) -> Option<&InterfaceSchema> {
        self.interfaces.get(&interface_id)
    }

    pub fn structure(&self, struct_id: u64) -> Option<&StructSchema> {
        self.structs.get(&struct_id)
    }

    pub fn program_data(&self, program_id: u64) -> Option<u64> {
        self.program_data.get(&program_id).copied()
    }

    pub fn method(&self, interface_id: u64, method_id: u16) -> Option<ResolvedMethod<'_>> {
        let interface = self.interfaces.get(&interface_id)?;
        let method = interface.methods.get(method_id as usize)?;
        Some(ResolvedMethod { interface, method })
    }

    pub fn ancestors(&self, interface_id: u64) -> Vec<u64> {
        let mut ancestors = Vec::new();
        let mut pending = vec![interface_id];
        while let Some(current) = pending.pop() {
            let Some(interface) = self.interfaces.get(&current) else {
                continue;
            };
            for superclass in &interface.superclasses {
                if *superclass != interface_id && !ancestors.contains(superclass) {
                    ancestors.push(*superclass);
                    pending.push(*superclass);
                }
            }
        }
        ancestors
    }

    fn score(&self, programs: &[ProgramVersion]) -> usize {
        programs
            .iter()
            .map(
                |program| match self.program_versions.get(&program.program_id) {
                    None => 0,
                    Some(version) => {
                        1 + usize::from(!version.is_empty() && *version == program.version)
                            + usize::from(same_revision(
                                &self.manifest.git_revision,
                                &program.git_revision,
                            ))
                    }
                },
            )
            .sum()
    }

    fn newer_than(&self, other: &Bundle) -> Ordering {
        compare_versions(&self.manifest.version, &other.manifest.version)
            .then_with(|| self.manifest.git_revision.cmp(&other.manifest.git_revision))
            .then_with(|| self.name.cmp(&other.name))
    }
}

pub struct SchemaRegistry {
    bundles: Vec<Arc<Bundle>>,
}

impl SchemaRegistry {
    fn new(mut bundles: Vec<Bundle>) -> anyhow::Result<SchemaRegistry> {
        if bundles.is_empty() {
            bail!("no schema bundles to load");
        }
        bundles.sort_by(|left, right| right.newer_than(left));
        Ok(SchemaRegistry {
            bundles: bundles.into_iter().map(Arc::new).collect(),
        })
    }

    pub fn load_directory(path: &Path) -> anyhow::Result<SchemaRegistry> {
        let mut bundles = Vec::new();
        let entries = std::fs::read_dir(path)
            .with_context(|| format!("read the schema directory {}", path.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("list the schema directory {}", path.display()))?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(".capnp.bin") else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let bytes = std::fs::read(entry.path())
                .with_context(|| format!("read the schema bundle {}", entry.path().display()))?;
            let manifest_path = path.join(format!("{name}.json"));
            let manifest_text = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("read the bundle manifest {}", manifest_path.display()))?;
            let manifest: BundleManifest =
                serde_json::from_str(&manifest_text).with_context(|| {
                    format!("parse the bundle manifest {}", manifest_path.display())
                })?;
            bundles.push(Bundle::parse(name, &bytes, manifest)?);
        }
        let registry = SchemaRegistry::new(bundles)
            .with_context(|| format!("load schema bundles from {}", path.display()))?;
        tracing::info!(
            directory = %path.display(),
            bundles = registry.bundles.len(),
            newest = %registry.bundles[0].name,
            "schema bundles loaded"
        );
        Ok(registry)
    }

    pub fn from_bundles(bundles: Vec<Vec<u8>>) -> anyhow::Result<SchemaRegistry> {
        let parsed = bundles
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                Bundle::parse(&format!("bundle-{index}"), bytes, BundleManifest::default())
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        SchemaRegistry::new(parsed)
    }

    pub fn bundles(&self) -> &[Arc<Bundle>] {
        &self.bundles
    }

    pub fn bundle_for(&self, programs: &[ProgramVersion]) -> Arc<Bundle> {
        let mut best = &self.bundles[0];
        let mut best_score = best.score(programs);
        for bundle in &self.bundles[1..] {
            let score = bundle.score(programs);
            if score > best_score {
                best = bundle;
                best_score = score;
            }
        }
        if best_score == 0 && !programs.is_empty() {
            tracing::warn!(
                bundle = %best.name,
                programs = programs.len(),
                "no schema bundle knows any of the node's programs; using the newest bundle"
            );
        } else if best_score < programs.len() * 3 {
            tracing::info!(
                bundle = %best.name,
                score = best_score,
                exact = programs.len() * 3,
                "no schema bundle matches the node's programs exactly; using the closest one"
            );
        }
        best.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp::traits::HasTypeId;
    use dusk_capnp::dusk_capnp::{dusk, portal, process, stream};

    fn tree() -> SchemaRegistry {
        SchemaRegistry::load_directory(Path::new(env!("NIGHTFALL_TREE_SCHEMAS"))).unwrap()
    }

    fn interface_named<'a>(bundle: &'a Bundle, name: &str) -> &'a InterfaceSchema {
        bundle
            .interfaces
            .values()
            .find(|interface| interface.name == name)
            .unwrap_or_else(|| panic!("no interface named {name}"))
    }

    #[test]
    fn names_interfaces_and_methods_without_the_file_prefix() {
        let registry = tree();
        let bundle = registry.bundle_for(&[]);
        let dusk_id = <dusk::Client as HasTypeId>::TYPE_ID;
        let resolved = bundle.method(dusk_id, 0).unwrap();
        assert_eq!(resolved.action(), "Dusk.process");
        assert_eq!(bundle.method(dusk_id, 10).unwrap().action(), "Dusk.dusk");
        assert_eq!(
            bundle.method(dusk_id, 9).unwrap().action(),
            "Dusk.namespaceId"
        );
        assert_eq!(
            bundle.method(dusk_id, 11).unwrap().action(),
            "Dusk.fleetToken"
        );
        assert!(bundle.method(dusk_id, 12).is_none());
        assert_eq!(
            interface_named(&bundle, "LogsArgs.Server").methods[0].name,
            "openStream"
        );
        assert_eq!(interface_named(&bundle, "ShPortal").name, "ShPortal");
    }

    #[test]
    fn detects_streaming_methods() {
        let bundle = tree().bundle_for(&[]);
        let stream_id = <stream::Client as HasTypeId>::TYPE_ID;
        assert!(bundle.method(stream_id, 0).unwrap().method.streaming);
        assert!(!bundle.method(stream_id, 1).unwrap().method.streaming);
        assert!(
            !bundle
                .method(<dusk::Client as HasTypeId>::TYPE_ID, 2)
                .unwrap()
                .method
                .streaming
        );
    }

    #[test]
    fn follows_superclasses() {
        let bundle = tree().bundle_for(&[]);
        let shell_portal = interface_named(&bundle, "ShPortal");
        let output_portal = interface_named(&bundle, "OutputPortal");
        let portal_id = <portal::Client as HasTypeId>::TYPE_ID;
        let ancestors = bundle.ancestors(shell_portal.id);
        assert!(ancestors.contains(&portal_id));
        assert!(ancestors.contains(&output_portal.id));
        assert!(!ancestors.contains(&shell_portal.id));
        assert_eq!(
            bundle.method(portal_id, 0).unwrap().action(),
            "Portal.programId"
        );
    }

    #[test]
    fn marks_sensitive_fields_and_parameters() {
        let bundle = tree().bundle_for(&[]);
        let kvs_portal = interface_named(&bundle, "KvsPortal");
        let set = &kvs_portal.methods[1];
        assert_eq!(set.name, "set");
        let parameters = bundle.structure(set.param_struct).unwrap();
        let value = parameters
            .fields
            .iter()
            .find(|field| field.name == "value")
            .unwrap();
        let key = parameters
            .fields
            .iter()
            .find(|field| field.name == "key")
            .unwrap();
        assert!(value.sensitive);
        assert!(!key.sensitive);
        let credential = bundle
            .structs
            .values()
            .find(|structure| structure.name == "Credential")
            .unwrap();
        let sensitive: Vec<&str> = credential
            .fields
            .iter()
            .filter(|field| field.sensitive)
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(sensitive, ["fleetToken", "installToken"]);
    }

    #[test]
    fn finds_one_pipeline_path_per_interface_slot() {
        let bundle = tree().bundle_for(&[]);
        let dusk_id = <dusk::Client as HasTypeId>::TYPE_ID;
        let process_method = bundle.method(dusk_id, 0).unwrap().method;
        assert_eq!(process_method.pipeline_paths.len(), 1);
        assert_eq!(process_method.pipeline_paths[0].pointers, [0]);
        assert_eq!(
            process_method.pipeline_paths[0].interface_id,
            <process::Client as HasTypeId>::TYPE_ID
        );
        assert_eq!(
            process_method.pipeline_paths[0].structs,
            [process_method.result_struct]
        );
        assert!(
            bundle
                .method(dusk_id, 2)
                .unwrap()
                .method
                .pipeline_paths
                .is_empty()
        );
        let portal_method = bundle
            .method(<process::Client as HasTypeId>::TYPE_ID, 5)
            .unwrap()
            .method;
        assert_eq!(portal_method.name, "portal");
        assert_eq!(portal_method.pipeline_paths.len(), 1);
        assert!(
            bundle
                .method(<stream::Client as HasTypeId>::TYPE_ID, 0)
                .unwrap()
                .method
                .pipeline_paths
                .is_empty()
        );
    }

    #[test]
    fn maps_program_ids_to_their_args_data() {
        let bundle = tree().bundle_for(&[]);
        let sleep_data = bundle.program_data(0xc0da_1e11_ed8d_4a36).unwrap();
        assert_eq!(bundle.structure(sleep_data).unwrap().name, "SleepArgs.Data");
        assert!(bundle.program_data(0x1234).is_none());
    }

    fn single_program_bundle(
        name: &str,
        version: &str,
        git_revision: &str,
        program_version: &str,
    ) -> Bundle {
        let bytes = std::fs::read(env!("NIGHTFALL_TREE_BUNDLE")).unwrap();
        Bundle::parse(
            name,
            &bytes,
            BundleManifest {
                version: version.to_string(),
                git_revision: git_revision.to_string(),
                programs: vec![ManifestProgram {
                    program_id: "c0da1e11ed8d4a36".to_string(),
                    name: "sleep".to_string(),
                    version: program_version.to_string(),
                }],
            },
        )
        .unwrap()
    }

    #[test]
    fn picks_the_bundle_matching_the_node_and_falls_back_to_the_newest() {
        let registry = SchemaRegistry::new(vec![
            single_program_bundle("old", "0.1.0", "aaaaaaaaaaaaaaaa", "0.1.0"),
            single_program_bundle("new", "0.2.0", "bbbbbbbbbbbbbbbb", "0.2.0"),
            single_program_bundle("middle", "0.1.5", "cccccccccccccccc", "0.1.5"),
        ])
        .unwrap();
        assert_eq!(registry.bundles()[0].name(), "new");
        let sleep = |version: &str, git_revision: &str| ProgramVersion {
            program_id: 0xc0da_1e11_ed8d_4a36,
            version: version.to_string(),
            git_revision: git_revision.to_string(),
        };
        assert_eq!(
            registry
                .bundle_for(&[sleep("0.1.0", "aaaaaaaaaaaaaaaa")])
                .name(),
            "old"
        );
        assert_eq!(
            registry
                .bundle_for(&[sleep("0.1.5", "cccccccccccccccc")])
                .name(),
            "middle"
        );
        assert_eq!(
            registry
                .bundle_for(&[sleep("0.1.5", "dddddddddddddddd")])
                .name(),
            "middle"
        );
        assert_eq!(
            registry
                .bundle_for(&[sleep("9.9.9", "eeeeeeeeeeeeeeee")])
                .name(),
            "new"
        );
        assert_eq!(registry.bundle_for(&[]).name(), "new");
    }

    #[test]
    fn refuses_input_that_is_not_a_bundle() {
        assert!(SchemaRegistry::from_bundles(vec![]).is_err());
        assert!(SchemaRegistry::from_bundles(vec![vec![1, 2, 3]]).is_err());
        let bytes = std::fs::read(env!("NIGHTFALL_TREE_BUNDLE")).unwrap();
        assert_eq!(
            SchemaRegistry::from_bundles(vec![bytes])
                .unwrap()
                .bundles()
                .len(),
            1
        );
    }

    #[test]
    fn compares_versions_numerically() {
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert!(same_revision("e7ae8b449296a508", "e7ae8b449296a508"));
        assert!(!same_revision("", ""));
        assert!(!same_revision("e7ae8b449296a508", "0000000000000000"));
    }
}
