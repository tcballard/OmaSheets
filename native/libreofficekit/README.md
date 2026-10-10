# Historical LibreOfficeKit reference

These C++ sources are retained for REA contract investigation only. The product
launcher, native bundle, installer and Arch package do not build or ship them.
OmaSheets uses its own Rust kit/service and Qt grid.

The separate `rea-reference` job in `.github/workflows/native-kit.yml` compiles
`lok_render.cpp` against an installed LibreOffice reference, checks one bounded
rendering scenario with REA and retains the raw evidence. This reference job is
not a runtime or packaging dependency. See
[`docs/NATIVE-KIT-INVESTIGATION.md`](../../docs/NATIVE-KIT-INVESTIGATION.md).
