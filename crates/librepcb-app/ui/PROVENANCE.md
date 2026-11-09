# Provenance of the UI sources

The `.slint` files in this directory are upstream LibrePCB's Slint UI,
ported source of this repository from now on (see `docs/ui-design.md`,
decision 1). They are compiled by this crate (`librepcb-app-ui`,
`build.rs`) and driven by the Rust backend in `crates/librepcb-app/src`.

## Upstream revision

- LibrePCB: <https://github.com/LibrePCB/LibrePCB>, commit
  `7f1b548abb61bd59b2b9c1166eca3e6557480c17` (2026-09-26), directory
  `libs/librepcb/ui/**` (all `.slint` files; `CMakeLists.txt` not copied).
- Resources referenced by the `.slint` files (`@image-url(...)` and font
  imports), copied to `resources/<path relative to the upstream root>`:
  - `img/**` from the LibrePCB repository (same commit);
  - `libs/font-awesome/svgs/**` from the submodule
    <https://github.com/FortAwesome/Font-Awesome> at
    `af620534bfc3c2d4cbefcfeec29603bbe7809e64`;
  - `libs/bootstrap-icons/icons/**` from the submodule
    <https://github.com/twbs/icons> at
    `31673a7249bf9043f065aa5d263fc454946de3d8`;
  - `share/librepcb/fonts/*.ttf` (Noto Sans) from the submodule
    <https://github.com/LibrePCB/librepcb-fonts> at
    `a1ab3c1a1f82cada3ff8477bd6d368f6d56fe5b0`.
- Additional resources used by the Rust backend (not referenced by
  `.slint` files): `img/app/librepcb.png`, `img/places/*.png`.

Only the files actually referenced are copied. Refresh them with
`scripts/import-upstream-ui.py` (see its documentation; it overwrites the
`.slint` files, so re-apply the local changes below).

## Slint version

Upstream builds against the Slint submodule `libs/slint` at commit
`a7e91481f1dec2503c4b2123494a767b7c689978` (version 1.18.0, development
snapshot) with the `cosmic-dark` style. This workspace pins Slint master
`aebe3dd580ce0594de272bc5797b7444a2aa18fd` (1.20.0 development snapshot),
including `slint-build` and the canvas adapter. UI builds select edition 1.20
and retain the same style.

## Licenses

- `.slint` files and `img/**`: GPL-3.0-or-later like LibrePCB (a few
  images in upstream's `img/` come from other free sources, see upstream's
  `.reuse/dep5` and `LICENSES/`).
- Font Awesome Free icons: CC BY 4.0 (`resources/libs/font-awesome/LICENSE.txt`).
- Bootstrap Icons: MIT (`resources/libs/bootstrap-icons/LICENSE`).
- Noto Sans fonts: SIL Open Font License 1.1
  (`resources/share/librepcb/fonts/LICENSE`).

## Local changes

1. Resource paths rewritten from the upstream repository layout
   (`../../font-awesome/...`, `../../../img/...`) to
   `resources/<path relative to the upstream root>` (done by the import
   script).
2. `api/data.slint`: removed the field `author-default` from the preview
   sample data of `CreateLibraryTabData` (the struct has no such field;
   Slint 1.18.1 warns about it).
3. License files of the icon sets and fonts added under `resources/`.
4. Embedded MCP server (librepcb-rs only): `api/data.slint` properties
   `mcp-server-running`, `mcp-server-url`, `mcp-agent-active`,
   `mcp-agent-status`, `mcp-follow-agent`; `api/backend.slint` callback
   `toggle-mcp-server()`; `statusbar.slint` button with popup (enable
   the server, "show what the agent edits") left of the notifications
   button.
5. Scene editing (M3a, librepcb-rs only): `project/sceneeditor.slint`
   (scene context menus; upstream opens `QMenu`s from the editor states),
   shown by `project/schematic/schematictab.slint` and
   `project/board/board2dtab.slint`.
6. Project dialogs (M3b, librepcb-rs only; upstream uses Qt Widgets
   dialogs): `dialogs/formdialog.slint` (the generic form dialog of the
   properties, rename, setup, export and output jobs dialogs, built from
   upstream's widgets) and `dialogs/addcomponentdialog.slint` (port of
   upstream's `addcomponentdialog.ui`), shown as overlays by
   `appwindow.slint` and exported by `ui.slint`.
7. `widgets/treeview.slint`: `current-index` is `in-out` (was private) so
   the backend can select a row (the "add component" dialog selects the
   first device found).
8. Scene context menus of the library element editors (M4b, librepcb-rs
   only): `library/sym/symboltab.slint` and `library/pkg/packagetab.slint`
   show `project/sceneeditor.slint`'s `SceneContextMenu` like the
   schematic and board tabs (upstream opens `QMenu`s from the editor
   states).
9. `library/pkg/packageeditortagspanel.slint`: the "new tag" line edit has
   a fixed `border-radius` (half its preferred height) instead of
   `self.height / 2`, which is a runtime binding recursion with Slint 1.18.1
   (the layout of the panel needs the border radius).
10. Color scheme editor (M3c, librepcb-rs only): in `colorschemedialog.slint`
    the upstream `ColorSchemeDialog` window is the component
    `ColorSchemePanel` (a `Rectangle`: no `title`, `icon` and
    `default-font-family`), shown as overlay by the new
    `ColorSchemeOverlay` with the `ColorSchemeEditor` global (upstream opens
    a separate window; the headless platform has one window only);
    `appwindow.slint` shows the overlay above the form dialogs, `ui.slint`
    exports the global and the data types instead of the window.
