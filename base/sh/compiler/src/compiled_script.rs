use capnp::message::{Builder, HeapAllocator};
use capnp::private::layout::CapTable;
use capnp::traits::{Imbue, ImbueMut};

use crate::bytecode_capnp::bytecode;

pub struct CompiledScript {
    message: Builder<HeapAllocator>,
    capabilities: CapTable,
}

impl CompiledScript {
    pub fn from_reader(script: bytecode::Reader<'_>) -> capnp::Result<Self> {
        let mut message = Builder::new(HeapAllocator::new());
        let mut capabilities = CapTable::new();
        {
            let mut root: capnp::any_pointer::Builder = message.get_root()?;
            root.imbue_mut(&mut capabilities);
            root.set_as(script)?;
        }
        Ok(Self {
            message,
            capabilities,
        })
    }

    pub fn root(&self) -> capnp::Result<bytecode::Reader<'_>> {
        let mut root = self.message.get_root_as_reader::<bytecode::Reader<'_>>()?;
        root.imbue(&self.capabilities);
        Ok(root)
    }
}
