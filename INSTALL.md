# Install OmaSheets v0.0.1 (development preview)

OmaSheets currently targets Omarchy `quattro`. It needs LibreOffice Calc,
Bubblewrap, a system Python with LibreOffice's UNO module, and `pipx`.

```bash
sudo pacman -S --needed libreoffice-fresh python-uno bubblewrap python-pipx
pipx install 'git+https://github.com/tcballard/OmaSheets.git@feat/v0.0.1'
omasheets integrate install
omarchy plugin add https://github.com/tcballard/OmaSheets.git --enable
```

`omasheets integrate install` operates only on the current user's desktop
entry and `mimeapps.list`. It records the exact files it created or changed.
The corresponding removal command restores untouched files and preserves a
desktop entry that another process edited:

```bash
omasheets integrate uninstall
omarchy plugin remove io.github.tcballard.omasheets
pipx uninstall omasheets
```

Open a workbook normally from the file manager, or select one for constrained
agent access:

```bash
omasheets open ./book.xls
omasheets select ./book.xlsx
```

The bar widget can open the selected workbook and display staged-plan status.
Its review button opens `omasheets review-current` in a terminal. Publication
still requires the exact local approval token; the widget and MCP server do not
possess commit authority.
