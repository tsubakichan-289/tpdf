# tpdf

`tpdf` is a small Rust PDF viewer that renders directly inside Ghostty. PDFium
rasterizes each page to RGBA pixels in a worker thread; `tpdf` sends those pixels
to the terminal with the Kitty graphics protocol. It does not open a GUI window,
run `pdftoppm`, or create temporary PNG files.

## Install

PDFium is a runtime dependency and is not bundled in the executable. Download the
matching shared library from
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases),
then either:

- put `libpdfium.so` (Linux), `libpdfium.dylib` (macOS), or `pdfium.dll`
  (Windows) beside the `tpdf` executable; or
- install it somewhere the platform dynamic loader searches.

Then build:

```bash
cargo build --release
```

The executable is `target/release/tpdf`. Copy it to a directory in `PATH` if
desired.

## Usage

```bash
tpdf document.pdf
tpdf document.pdf --page 37
tpdf document.pdf --zoom 125
tpdf document.pdf --no-watch
```

Watching is enabled by default. `--page` is one-based. `--zoom` is a percentage;
without it the page is fitted to the pane.

## Typst

Run the compiler and viewer in separate Ghostty or zellij panes:

```bash
typst watch thesis.typ
```

```bash
tpdf thesis.pdf
```

The parent directory is watched rather than the PDF inode, so Typst/LaTeX atomic
rename and replace workflows are handled. Changes are debounced and transient
load failures are retried after 50, 100, and 200 ms. Reloading preserves the
current page, clamping it only if the new document is shorter.

## Key bindings

| Key | Action |
|---|---|
| `j`, `Down`, `PageDown`, `Ctrl-d` | Next page |
| `k`, `Up`, `PageUp`, `Ctrl-u` | Previous page |
| `g`, `Home` | First page |
| `G`, `End` | Last page |
| `+`, `=` | Zoom in |
| `-` | Zoom out |
| `0` | Fit to window |
| `h`, `Ctrl-h` | Scroll left |
| `l` | Scroll right |
| `r`, `Ctrl-l` | Force redraw |
| `q`, `Ctrl-c` | Quit |

## Ghostty and the Kitty graphics protocol

`tpdf` transmits in-memory 32-bit RGBA data in protocol-compliant base64 chunks.
A completed image is placed before the previous image is deleted, reducing white
flashes during reloads. Terminal pixel dimensions are read on resize and the
current page is rasterized again. Terminals not identifiable as Ghostty, Kitty,
or WezTerm are rejected; the hidden `--force` flag is available for compatible
multiplexers that do not preserve identifying environment variables.

The terminal/multiplexer must pass Kitty graphics escape sequences through. A
zellij or tmux version/configuration that does not support the protocol cannot be
fixed by `tpdf`.

## Architecture

- `pdf/renderer.rs`: PDFium binding and the raster worker
- `terminal/kitty.rs`: pure Kitty encoding, image placement, deletion
- `event.rs`: blocking terminal input producer
- `watcher.rs`: non-recursive parent-directory watch
- `app.rs`: navigation, zoom, offsets, and three-page bitmap cache
- `ui/status.rs`: one-line status bar

The main loop blocks on a channel; there is no busy loop or async runtime. The
cache retains only the current, previous, and next bitmap for the active document
generation and render dimensions.

## Current limitations

- PDFium must be installed separately and its API build must be compatible with
  the `pdfium_latest` feature selected in `Cargo.toml`.
- Password-protected PDFs are not currently supported.
- Vertical scrolling is not exposed; at fixed zoom, an over-height page is shown
  from its top edge.
- Kitty graphics support is inferred from the environment rather than queried.
- Pages with different dimensions can require one corrective render after page
  navigation.
