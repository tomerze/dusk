use alloc::rc::Rc;
use core::cell::RefCell;

use dusk_capnp::capnp;
use dusk_capnp::capnp::any_pointer;
use dusk_capnp::capnp::capability::FromClientHook;
use dusk_capnp::capnp::message::HeapAllocator;
use dusk_capnp::capnp::traits::Owned;
use dusk_capnp::capnp_rpc::ImbuedMessageBuilder;
use dusk_capnp::dusk_capnp::created;
use dusk_capnp::dusk_capnp::program_args;

/// Wire-level type of `ProgramArgs` as it appears on `Dusk.process`.
pub type AnyProgramArgs = program_args::Owned<any_pointer::Owned, any_pointer::Owned>;

pub struct ProgramArgs {
    inner: RefCell<ImbuedMessageBuilder<HeapAllocator>>,
}

impl ProgramArgs {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(ImbuedMessageBuilder::new(HeapAllocator::new())),
        }
    }

    pub fn with_root_builder<R, F>(&self, f: F) -> capnp::Result<R>
    where
        F: for<'a> FnOnce(
            program_args::Builder<'a, any_pointer::Owned, any_pointer::Owned>,
        ) -> capnp::Result<R>,
    {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        f(root)
    }

    /// Copy a `ProgramArgs` reader into a new owned message. Capabilities
    /// embedded in the source are forwarded into the destination's cap table.
    pub fn from_reader(
        reader: program_args::Reader<'_, any_pointer::Owned, any_pointer::Owned>,
    ) -> capnp::Result<Rc<Self>> {
        let owned = Self::new();
        owned
            .inner
            .borrow_mut()
            .set_root::<AnyProgramArgs>(reader)?;
        Ok(Rc::new(owned))
    }

    /// Read the `programId` field.
    pub fn program_id(&self) -> capnp::Result<u64> {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        Ok(root.into_reader().get_program_id())
    }

    pub fn pid(&self) -> capnp::Result<Option<u64>> {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        match root.into_reader().get_pid().which()? {
            program_args::pid::Which::Auto(()) => Ok(None),
            program_args::pid::Which::Fixed(pid) => Ok(Some(pid)),
        }
    }

    /// Set a fixed pid or None for auto.
    pub fn set_pid(&self, pid: Option<u64>) -> capnp::Result<()> {
        let mut message_builder = self.inner.borrow_mut();
        let mut root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        match pid {
            Some(pid) => root.reborrow().get_pid().set_fixed(pid),
            None => root.reborrow().get_pid().set_auto(()),
        }
        Ok(())
    }

    pub fn created(&self) -> capnp::Result<Option<created::Client>> {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        let reader = root.into_reader();
        if !reader.has_created() {
            return Ok(None);
        }
        Ok(Some(reader.get_created()?))
    }

    pub fn set_created(&self, client: created::Client) -> capnp::Result<()> {
        let mut message_builder = self.inner.borrow_mut();
        let mut root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        root.set_created(client);
        Ok(())
    }

    /// Read the `args.data` slot as the typed reader of `T` and feed it into
    /// `f`.
    pub fn with_data<T, R, F>(&self, f: F) -> capnp::Result<R>
    where
        T: Owned,
        F: for<'a> FnOnce(<T as Owned>::Reader<'a>) -> capnp::Result<R>,
    {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        let data = root.into_reader().get_args().get_data()?;
        f(data.get_as::<<T as Owned>::Reader<'_>>()?)
    }

    pub fn data_owned<T: Owned>(&self) -> capnp::Result<capnp::message::TypedBuilder<T>> {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        let data_reader = root
            .into_reader()
            .get_args()
            .get_data()?
            .get_as::<<T as Owned>::Reader<'_>>()?;
        let mut owned = capnp::message::TypedBuilder::<T>::new_default();
        owned.set_root(data_reader)?;
        Ok(owned)
    }

    /// Extract `args.server` as the capability type `T`.
    pub fn server_as<T: FromClientHook>(&self) -> capnp::Result<T> {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        root.into_reader()
            .get_args()
            .get_server()?
            .get_as_capability::<T>()
    }

    /// Borrow the message as a reader inside a closure. Used to feed it into
    /// an outgoing RPC request's `program_args` field.
    pub fn with_reader<R, F>(&self, f: F) -> capnp::Result<R>
    where
        F: for<'a> FnOnce(
            program_args::Reader<'a, any_pointer::Owned, any_pointer::Owned>,
        ) -> capnp::Result<R>,
    {
        let mut message_builder = self.inner.borrow_mut();
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            message_builder.get_root()?;
        f(root.into_reader())
    }

    pub fn reader_owned(&self) -> capnp::Result<Rc<Self>> {
        self.with_reader(Self::from_reader)
    }
}

impl Default for ProgramArgs {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for ProgramArgs {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut message_builder = match self.inner.try_borrow_mut() {
            Ok(builder) => builder,
            Err(_) => return formatter.write_str("ProgramArgs { <borrowed> }"),
        };
        let root: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned> =
            match message_builder.get_root() {
                Ok(root) => root,
                Err(error) => {
                    return formatter.write_fmt(format_args!("ProgramArgs {{ <error: {error}> }}"));
                }
            };
        core::fmt::Debug::fmt(&root.into_reader(), formatter)
    }
}
