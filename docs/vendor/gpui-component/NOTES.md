# gpui-component 0.5.1 — API facts this app relies on

Verified against the v0.5.1 tag. Re-verify all of these on any version bump;
this crate is pre-1.0 and the known failure mode of generated code is calling
APIs from a different version.

## Bootstrap

- `gpui_component::init(cx)` must run before any component is used.
- The window root view must be `Root::new(child_view, window, cx)`.
- Icons/fonts come from the `gpui-component-assets` crate:
  `Application::new().with_assets(Assets)`.

## Overlay layers are NOT automatic

`Root` stores dialogs/sheets/notifications but its own render does not draw
them. The app's root view must include, after its normal children:

```rust
.children(Root::render_dialog_layer(window, cx))
.children(Root::render_notification_layer(window, cx))
```

Without these, `open_dialog` / `push_notification` silently do nothing.

## Dialogs and notifications

- The trait is `gpui_component::WindowExt` (on `Window`):
  `open_dialog`, `close_dialog`, `push_notification`, `open_sheet`, ...
- `open_dialog(cx, |dialog, window, cx| ...)` — builder closure re-runs per
  render. Dialog default width is 480px; `dialog.w(px(...))` sets it.

## Inputs

- `InputState::new(window, cx).placeholder(...).default_value(...)` held as
  an `Entity<InputState>`; render with `Input::new(&state)`.
- Read with `state.read(cx).value()`. Change events via
  `cx.subscribe_in(&state, window, ...)` on `InputEvent::Change`.
- Multi-line: `InputState::new(...).multi_line(true).rows(n)` (default 2
  rows); the rendered height can be forced with `Input::new(&state).h(px)`
  (multi-line only). `auto_grow(min, max)` and `code_editor(lang)` also
  exist. `Input` implements `Styled`, so `.font_family(...)` on a wrapper
  or the element styles the text.
- **Placeholders must be single-line.** A `\n` inside `placeholder(...)`
  panics gpui 0.2.2's Mac text layout the moment the empty input renders
  ("end byte index N is out of bounds for string of length M" in
  `text_system.rs`): the placeholder paints per-line but is laid out
  against the full string's byte length.

## Select (dropdown)

- State: `SelectState::new(delegate, selected_index, window, cx)` held as
  `Entity<SelectState<D>>`; `Vec<T>` and `SearchableVec<T>` (search box in
  the menu via `.searchable(true)`) are delegates for `T: SelectItem`
  (`String`, `SharedString`, `&'static str` provided; `Value = Self`).
- Select by value: `state.set_selected_value(&value, window, cx)` (needs
  `Value: PartialEq`); a value not in the list selects nothing and the
  trigger shows the placeholder.
- Read: `state.read(cx).selected_value() -> Option<&Value>`.
- Render: `Select::new(&state)` (+ `.placeholder(...)`, `.menu_width(...)`,
  `.cleanable(...)`). Works inside dialogs; the menu renders on the popover
  layer.

## Context menus

- `gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem}`; no
  extra init beyond `gpui_component::init`.
- `.context_menu(|menu, window, cx| menu.item(...))` (from `ContextMenuExt`,
  available on any `ParentElement + Styled` element) wraps the element; a
  right press inside its bounds opens the menu at the pointer.
- Items: `menu.item(PopupMenuItem::new("Label").disabled(bool)
  .on_click(|_, window, cx| ...))` and `menu.separator()`. The click handler
  is `Fn(&ClickEvent, &mut Window, &mut App)`; closure-based items need no
  gpui action types.
- The builder closure runs on each open, but in a `window.defer` one frame
  after the right press. Anything position-dependent (e.g. "what's under
  the pointer") must be captured by the wrapped element's own
  `on_mouse_down(MouseButton::Right, ...)` listener, which fires before the
  deferred build; the builder itself never sees the click position.

## Resizable panels

- `h_resizable(id)` / `v_resizable(id)` with `resizable_panel().size(px)
  .size_range(range).child(any_element)` children. State is internal,
  keyed on the element id.

## TitleBar

- `TitleBar::new().child(...)` renders children in a justify-between bar and
  window controls on Linux/Windows; on macOS it pads 80px for traffic lights.
- Height is fixed at 34px (`TITLE_BAR_HEIGHT`). Pass
  `TitleBar::title_bar_options()` as `WindowOptions.titlebar` so the macOS
  traffic-light position matches.

## Theme

- `Theme::change(ThemeMode::Dark | Light, Some(window), cx)` switches mode;
  after that, individual `cx.global_mut::<Theme>().colors.*` fields can be
  overridden (this app maps its own palette onto `background`, `primary`,
  `border`, `title_bar`, etc. in `src/theme.rs`).
- Components read colors via `cx.theme()` (`ActiveTheme` trait).

## gpui 0.2.2 interaction gotchas (app-level, learned building the shell)

- A parent's `on_mouse_down` fires after a child's during bubble; an overlay
  backdrop that closes on mouse-down will swallow clicks on its own popup
  children unless the popup calls `cx.stop_propagation()` on mouse-down.
- `FluentBuilder` (gpui prelude) already provides `.when()` / `.when_else()`.
- Entity constructors: `cx.new(...)` needs `use gpui::AppContext`.
