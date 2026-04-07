use anyhow::Result;

use crate::game::injection::{self, PatchState};
use crate::memory::access::MemoryAccess;

/// Request a stats-panel redraw on the next game loop iteration.
///
/// If the code-cave patch is active, this sets the dirty flag byte and
/// the game redraws on its next main-loop tick.  If no patch is active,
/// this is a no-op — we cannot force a redraw without the patch.
pub fn nudge_redraw(
    mem: &dyn MemoryAccess,
    _dos_base: usize,
    patch: Option<&PatchState>,
) -> Result<()> {
    if let Some(state) = patch {
        injection::trigger_redraw(mem, state)?;
    }
    Ok(())
}
