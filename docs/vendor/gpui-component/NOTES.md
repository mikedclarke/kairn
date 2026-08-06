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
