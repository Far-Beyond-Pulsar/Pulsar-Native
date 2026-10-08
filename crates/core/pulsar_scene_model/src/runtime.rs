//! This crate's process-wide state, shared by every statically linked copy
//! (Pulsar-Native#1083): the ordinal source behind generated object ids and
//! sibling order, and the registered motion gates. A plugin library's copy
//! is attached to the editor's when the library loads; see
//! `pulsar_scenedb::runtime` for the model.

use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use pulsar_scenedb::runtime::{AppendList, AttachError};

use crate::motion::MotionGate;

/// Version of [`Runtime`]'s ABI. Bump it on any change to `Runtime`'s fields
/// or to a type passed through them.
pub const ABI_VERSION: u64 = 1;

const FINGERPRINT: u64 = {
    let parts = [
        ABI_VERSION as usize,
        size_of::<Runtime>(),
        size_of::<MotionGate>(),
        size_of::<Registrations>(),
    ];
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < parts.len() {
        hash ^= parts[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
};

/// This crate's process-wide state, as the functions of the copy that owns
/// it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Runtime {
    /// [`ABI_VERSION`] and the layout fingerprint of the copy that built
    /// this runtime; [`attach`] refuses a runtime whose `abi` differs.
    pub abi: u64,
    pub(crate) next_ordinal: fn() -> u64,
    pub(crate) motion_gates: fn() -> &'static [&'static MotionGate],
    pub(crate) add_registrations: fn(&Registrations),
}

/// The registrations one copy's `inventory` collected.
pub struct Registrations {
    motion_gates: Vec<&'static MotionGate>,
}

static NEXT_ORDINAL: AtomicU64 = AtomicU64::new(1);
static MOTION_GATES: AppendList<MotionGate> =
    AppendList::new(|| inventory::iter::<MotionGate>.into_iter().collect());

static OWN: Runtime = Runtime {
    abi: FINGERPRINT,
    next_ordinal: || NEXT_ORDINAL.fetch_add(1, Ordering::Relaxed),
    motion_gates: || MOTION_GATES.get(),
    add_registrations: |registrations| MOTION_GATES.extend(&registrations.motion_gates),
};

static ATTACHED: AtomicPtr<Runtime> = AtomicPtr::new(std::ptr::null_mut());

/// The runtime this copy uses: the one it is attached to, or its own.
#[inline]
pub(crate) fn runtime() -> &'static Runtime {
    let attached = ATTACHED.load(Ordering::Acquire);
    if attached.is_null() {
        &OWN
    } else {
        // SAFETY: `attach` only stores a pointer to a `Runtime` that lives
        // for the process, checked for this ABI.
        unsafe { &*attached }
    }
}

/// The runtime to hand a plugin library's copy of this crate.
pub fn shared() -> &'static Runtime {
    runtime()
}

/// Attach this copy to `owner`, the runtime of the copy that owns the
/// process's state, and hand it this copy's motion gates. See
/// `pulsar_scenedb::runtime::attach` for the rules.
///
/// # Safety
///
/// `owner` is null or points to a [`Runtime`] that lives for the rest of
/// the process, and whose functions stay loaded that long.
pub unsafe fn attach(owner: *const Runtime) -> Result<(), AttachError> {
    // SAFETY: the caller's contract.
    let owner = unsafe { owner.as_ref() }.ok_or(AttachError::Null)?;
    if owner.abi != OWN.abi {
        return Err(AttachError::Abi {
            expected: OWN.abi,
            found: owner.abi,
        });
    }
    if std::ptr::eq(owner, &OWN) {
        return Ok(());
    }
    if NEXT_ORDINAL.load(Ordering::Relaxed) != 1 {
        return Err(AttachError::AlreadyInUse);
    }
    let owner_ptr = owner as *const Runtime as *mut Runtime;
    match ATTACHED.compare_exchange(
        std::ptr::null_mut(),
        owner_ptr,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {}
        Err(current) if current == owner_ptr => return Ok(()),
        Err(_) => return Err(AttachError::AlreadyAttached),
    }
    (owner.add_registrations)(&Registrations {
        motion_gates: inventory::iter::<MotionGate>.into_iter().collect(),
    });
    Ok(())
}
