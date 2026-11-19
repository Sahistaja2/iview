# iview

A fast picture viewer (Rust, egui). It opens quickly, decodes pictures straight to screen size and prefetches the neighbours.

(Code comments are in Estonian; the user interface, messages, command-line help and logs are in English.)

Developed with the help of Claude (Anthropic).

## Running

```powershell
cargo build --release
.\target\release\iview.exe D:\Pictures\IMG_0001.jpg   # a file (arrows walk through the folder)
.\target\release\iview.exe D:\Pictures                # a folder (first picture)
```

Options (`iview --help`):

| Option | What it does |
|---|---|
| `-h`, `--help`, `/?` | help |
| `-V`, `--version` | version |
| `-f`, `--fullscreen` | open in full screen (Esc closes) |
| `-n`, `--new-window` | new window instead of the already running viewer |
| `--config FILE` | settings file (same as `IVIEW_CONFIG`) |
| `--timing` | write timings to `startup.log` and `timing.log` next to the settings (same as `IVIEW_TIMING`) |
| `--` | only the path follows |

The path is read forgivingly: a path with spaces works without quotes, `"D:\Pictures\"` (Windows turns the trailing backslash into a quote) and relative paths work. A missing path or an unknown option prints a message (to the console, or a message box when started from Explorer) and exits with code 1 or 2; no empty window is opened. The release exe is a GUI program: PowerShell does not wait for it, so the `--help` text appears after the prompt.

### macOS: iview.app

```bash
tools/macos-app.sh                       # target/release/iview.app
tools/macos-app.sh --install             # and a copy in /Applications
IVIEW_UNIVERSAL=1 tools/macos-app.sh     # Apple Silicon + Intel in one binary
```

The script builds the binary (`--locked`) and assembles the bundle. The Info.plist template is `assets/macos/Info.plist`; it lists the picture formats for “Open With” and the folder-access usage texts. The program draws its own icon (`iview --export-iconset`, on Apple's icon grid) and `iconutil` turns it into `.icns`. The script signs the bundle (`IVIEW_SIGN`, otherwise the first developer certificate in the keychain, otherwise ad hoc) and registers it with LaunchServices.

**Opening from Finder** (double-click, “Open With”, `open -a iview file`, dropping a file on the Dock icon): Finder does not pass the file as a command-line argument but sends an “open documents” Apple Event. winit does not handle it, so `src/macos_open.rs` does. The launch file arrives before the window and goes straight to decoding, like a command-line file. If iview is already running, macOS sends the new file to the same process and it opens in the same window (the Mac “one window” behaviour). Running the binary directly from a terminal (`iview.app/Contents/MacOS/iview file`) takes the command-line argument as on Windows.

On Windows, `iview.exe` can be set as the default app for pictures (*Open with → Choose another app*). If the viewer is already running, a new file goes to the same window (*Settings → Opening → Single window*, on by default).

## Speed

Measured on this machine (NVIDIA RTX 3080, 2560 × 1440), 24 MP JPEG of 14 MB, release build:

| What | Time |
|---|---|
| Start until the window is visible with the picture (cold) | 435 ms |
| Same, warm (files and driver in the OS cache) | 360–410 ms |
| Second open while the viewer is running: the new process exits | 80 ms |
| … and the picture (144 MB uncompressed TIFF) is in the existing window | 34 ms |
| Next picture with the arrow key (prefetched) | 0 ms (texture is ready) |
| 24 MP JPEG decoded to screen size (one thread) | 140–165 ms |

Every start's stages are logged in `startup.log` next to the settings; with `IVIEW_TIMING=1` also `timing.log` (each decode and “picture on screen”).

### What makes it fast

- **Decoding straight to screen size** (`decode.rs`, `wic.rs`). JPEG, HEIC and TIFF are decoded by the Windows decoder (WIC), which scales down in the DCT stage. A 24 MP photo is never decoded at full size.
- **The camera's own JPEG from RAW files** (`raw.rs`, `cr3.rs`): CR3, CR2, NEF, ARW, DNG and others. The raw data is not touched. A setting can switch to the Windows codec instead.
- **Uncompressed TIFF** (panoramas, hundreds of MB): only every k-th row is read (`raw::tiff_rows_preview`).
- **Two decoder threads and a queue that is replaced on every move** (`loader.rs`). Skipped pictures are not decoded. The current picture always comes first. A big neighbour is not decoded at the same time as the current one: two 500 MB panoramas from the same disk would make the current one twice as slow.
- **1:1 and zoom**: the full size is decoded in the background; the old texture stays visible meanwhile.
- **Pictures without transparency go to the texture straight from RGB** (one copy less). Downscaling uses SIMD (`fast_image_resize`).
- **Own file stream for WIC** with a 4 MB buffer: the decoder seeks several times per row, and `BufReader` read the same chunk dozens of times. The stream also drives the progress bar.
- **Graphics driver warm-up in the background** (DX12 adapters), **DX12 and integrated GPU preferred**.
- **The window is born hidden and cloaked by DWM.** It becomes visible only once the picture has been drawn at the right size, so an empty or wrongly sized window is never seen. The window-open animation is turned off.
- **No disk or EXIF query runs on the UI thread.** A sleeping network drive does not freeze the window.
- **Decoding the first picture starts before the window.** `main` starts the loader right after reading the settings, sized for the window (for a maximized window, the screen). The picture and the graphics get ready in parallel: the log shows the picture decoded at ~160 ms and the window only at ~360 ms.
- **Single process** (named pipe). A second launch hands the file to the running window and exits, so no second process, graphics context or window is created. Explorer opens several selected files as several processes: they wait for the pipe for up to 3 s.
- **The original size comes from the decoder itself**, not from a separate header query: the file is not opened twice.
- **Prefetch in the direction of movement**: +1, +2, −1, +3, +4, −2. With *At the end of the folder: Wrap around*, prefetch also wraps over the end.
- In the dev build, dependencies (decoders) are optimized. Otherwise speed could not be judged at all.

## Keys

| Key | Action |
|---|---|
| `←` `→`, `PgUp` `PgDn`, Space, `Backspace` | previous / next (when the picture is larger than the window, arrows pan it) |
| `Home` `End` | first / last |
| wheel | previous / next (zoom, depending on settings); `Ctrl`+wheel does the other |
| `+` `−` | zoom (steps or 10 / 25 %) |
| `1` or `Ctrl+H`, `0` | 1:1, fit (this picture only) |
| `F` | window mode like IrfanView: toggle between *fit to screen* (picture fitted to the screen, small ones enlarged) and *picture 1:1* (window sized to the picture, no larger than the screen) |
| `Shift+F` | window mode *shrink big to screen*: big pictures fitted to the screen, small ones at 1:1 |
| `Shift+O` | window mode *picture 1:1* |
| `Shift+W` | window mode off: the window keeps its size, the picture is fitted into it |
| double-click | fit ↔ 1:1 |
| left drag | selection box. Once there is one, dragging outside it pulls the nearest edge or corner to the pointer and dragging inside moves it, so missing an edge never loses the selection (in IrfanView it does); `Ctrl`+drag keeps the ratio (`Shift+C`); **New** or `Esc` (or `Ctrl`+click) removes it to draw a new one |
| right drag (or middle) | pan; inside the selection it moves the selection |
| `Ctrl+Y` | crop to the selection |
| `Shift+C` | selection ratio: Free, 1:1, 3:2, 4:3, 16:9, 16:10, 21:9 (2.370:1), 2:3, 3:4, 9:16, 10:16 or your own (`w:h`); `Tab` moves between them, `Enter` or **OK** applies; makes the largest selection of that ratio |
| `Enter`, `F11` | full screen (text in a corner, template in settings); `Esc` back |
| `R` `L` | rotate clockwise / counter-clockwise |
| `H` `V` | mirror horizontally / vertically |
| `Ctrl+R` | downsize: longer side (presets, ½); `Ctrl+Enter` applies |
| `Ctrl+Z` | undo a change |
| `S` or `Ctrl+S` | save a copy as JPEG (name from template, quality from the original, EXIF and colour profile kept, orientation reset, file time from the original) |
| `Ctrl+C` | the selection, or the whole picture, to the clipboard as shown; for an unchanged picture without a selection also the file itself |
| `Ctrl+V` | clipboard picture into the list (in memory only, `Ctrl+S` saves it) |
| `F2` | rename (sidecar files such as `.xmp` follow) |
| `Del` | to the Recycle Bin (sidecar files follow) |
| `F4` or `Shift+1` | editor (setting; empty = the Windows “Edit” action) |
| `F5` or `Shift+R` | reload the file from disk |
| `G` | pause / resume an animated GIF or WebP |
| `O` or `Ctrl+O`, drop on the window | open a file or folder |
| `M` | minimize the window |
| `P` | settings |
| `Esc` | close |

Changes (rotate, mirror, crop, downsize) live only in the window. The file stays untouched until `S` writes a copy.

## Settings

`config.toml` is human-readable and holds the same content as the settings window (`P`). It lives in `%APPDATA%\iview\` on Windows and in `~/.config/iview/` on macOS and Linux. Settings are saved by comparing the whole configuration. The window position is stored in physical pixels and is restored correctly on a second monitor too.

The window mode (`F`, `Shift+F`, `Shift+O`, `Shift+W`, or *Window size* in the settings) is remembered and applies to every picture you open, as IrfanView's display options do. The window gets the picture's size on the current monitor and is centred there; rotating or resizing the picture resizes the window too. A window you maximized yourself stays maximized when you change pictures, but pressing a window-mode key restores it. In full screen only the zoom follows the mode. Resizing the window is Windows-only for now.

For testing: `IVIEW_CONFIG=<file>` gives a separate profile. It does not touch the user's settings or window and does not send files to the user's window (single process is per profile).

## Development

```powershell
cargo test
cargo clippy --all-targets
```

## Structure

```
main.rs       startup order (single process, loader before window, DX12)
app.rs        window: keys, drawing, boxes (F2, Ctrl+R, Ctrl+S), full screen, window birth
loader.rs     two decoder threads, queue, textures, animated pictures
decode.rs     single entry: RAW → uncompressed TIFF → WIC → image; SVG; downscaling
wic.rs        Windows decoder (DCT scaling, own file stream, progress)
raw.rs cr3.rs jpeg.rs   camera JPEG from the container, JPEG header and quality
exif.rs       EXIF (info bar, full-screen text, Ctrl+S)
save.rs       edits (rotate, mirror, longer side) and saving a copy
folder.rs     folder listing, natural order, sibling folder
template.rs   text templates ({name}, {taken:%d.%m.%Y} …)
settings.rs   settings window
platform.rs   single process, window cloak and position, clipboard, editor
macos_open.rs macOS: Finder “open documents” Apple Event
startup.rs    timing (startup.log, timing.log)
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
