use dusk_build::CapnpDep;

const COMMON: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/otlp/common.capnp"),
    crate_name: "dusk_program_logs",
    schema_ids: &[0x8f3a2b1c5d6e4790],
};

const LOG_RECORD: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/otlp/log_record.capnp"),
    crate_name: "dusk_program_logs",
    schema_ids: &[0xd06f74de2a9b6c32],
};

const SPAN: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/otlp/span.capnp"),
    crate_name: "dusk_program_logs",
    schema_ids: &[0xa3f59c1e7b8d4602],
};

const SH: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/../sh/capnp/sh.capnp"),
    crate_name: "dusk_program_sh",
    schema_ids: &[0xb25a041190c0e845],
};

fn main() {
    dusk_build::build(&[
        ("capnp/otlp/common.capnp", &[]),
        ("capnp/otlp/log_record.capnp", &[COMMON]),
        ("capnp/otlp/span.capnp", &[COMMON]),
        ("capnp/logs.capnp", &[SH, LOG_RECORD, SPAN, COMMON]),
    ]);
    if std::env::var_os("CARGO_FEATURE_C_API").is_some() {
        println!(
            "cargo::metadata=c_api_include={}",
            concat!(env!("CARGO_MANIFEST_DIR"), "/include")
        );
    }
}
