{{capnp-file-id}};

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = {{program-id}};

const resultTypeId :UInt64 = {{result-type-id}};

struct {{program-name | pascal_case}}Options {}

interface {{program-name | pascal_case}}Args extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :{{program-name | pascal_case}}Options);
}

interface {{program-name | pascal_case}}Portal extends(Dusk.Portal, Sh.OutputPortal) {}
