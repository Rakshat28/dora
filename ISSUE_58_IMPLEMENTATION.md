# Issue #58: Code View Pane Implementation

## Design Rationales

### 1. Why `auto_scrolled_for` is Reset in `select_next`/`select_prev` Rather Than in `auto_scroll_to_first_match` Itself

`auto_scrolled_for` tracks which file the code pane last auto-scrolled to. It must be reset **before** calling `auto_scroll_to_first_match()` so the method knows that this is a new file requiring an auto-scroll.

If reset inside `auto_scroll_to_first_match()`:
- Potential infinite recursion if auto-scroll calls selection change logic
- The method would lose its "idempotent" property (calling it twice for same file should not scroll again)
- Navigation logic would become fragmented

By resetting it in `select_next`/`select_prev`:
- Clear responsibility: navigation methods set up the preconditions for auto-scroll
- `auto_scroll_to_first_match()` remains a pure, idempotent check-and-scroll operation
- The flow is explicit: change selection → reset auto-scroll state → load source → auto-scroll

### 2. Why `ASSUMED_PANE_HEIGHT` is a Constant Rather Than the Real Pane Height

The code pane height is not known during AppState initialization because:
- Terminal size varies at runtime and changes with window resize events
- `auto_scroll_to_first_match()` is called in the event loop before rendering, before the frame size is available
- Computing the scroll offset requires a reasonable viewport height estimate

`ASSUMED_PANE_HEIGHT = 30` is:
- A conservative estimate: most terminals and editors show 30-40 lines of code
- Used only as a fallback for the vertical centering calculation
- The actual rendering in `draw_code_pane()` clamps the scroll offset to the real visible height anyway
- Result: first match appears centered (±10 lines) regardless of actual pane height

At render time, `effective_offset = state.scroll_offset.min(max_scroll)` applies hard clamping, so an oversized offset from assumption never causes out-of-bounds rendering.

### 3. Why the Render Function Does Not Mutate State for Scroll Clamping

The render function is called per frame and must remain **pure** (read-only on state) for these reasons:

- **Idempotency**: Rendering the same state multiple times produces identical output
- **Separation of concerns**: Logic (state mutation) happens in event handlers; presentation happens in render
- **Frame-based consistency**: If render could mutate state, multiple renders per frame could produce inconsistent output
- **Testing**: Pure render functions are easier to test and reason about

Instead of mutating state:
```rust
let effective_offset = state.scroll_offset.min(max_scroll);
```
The effective offset is computed locally in `draw_code_pane()` for rendering only. If the user scrolls below the file end, `state.scroll_offset` remains high, but rendering always clamps it. This way:
- State truly reflects user intent (they scrolled far down)
- Rendering stays safe and bounded
- The next file selection still respects the prior scroll position intention if applicable

### 4. Why Tab Cycles Through Three Panes Rather Than Toggling Between Two

A three-pane cycle supports future extensibility (Issue #59 adds AST View) and maintains consistent navigation:

- **Two-pane toggle**: FileTree ↔ CodeView would require rework when AstView is added later
- **Three-pane cycle**: FileTree → CodeView → AstView → FileTree is forward-compatible
  - Adding a fourth pane later requires no change to existing Tab logic
  - Users develop a muscle memory habit (Tab always cycles forward)
  - Each pane gets equal keyboard accessibility

Shift+Tab could be added later for reverse cycling without breaking this design.

---

## Implementation Summary

- **Added fields to AppState**: `cached_source` (HashMap), `auto_scrolled_for` (Option), `active_pane` (PaneId enum)
- **Cache management**: SOURCE_CACHE_SIZE = 10, oldest entry evicted when full
- **Auto-scroll logic**: Centers first match vertically using ASSUMED_PANE_HEIGHT = 30
- **Code rendering**: Displays line numbers (dim), match indicators (▶), syntax highlighting (reversed), and scrollbar
- **Pane focus**: Active pane border is bold; Tab cycles FileTree → CodeView → AstView → FileTree
- **Keyboard controls**: j/k scroll code when CodeView active; select files when FileTree active
- **Status bar**: Context-aware hints reflecting active pane

All 311 tests pass. Zero comments in code as required.
