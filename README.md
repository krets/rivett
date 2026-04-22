# <img src="resources/icon.png" width="48" height="48" valign="middle"> Rivett

**Rust Image Vetting Tool** — A professional, high-performance image viewer designed for photographers, digital artists, and collectors who need to sort and vet large volumes of images quickly.

## 🚀 Key Features

- **Extreme Performance:** Native Rust + OpenGL ensures buttery smooth panning and zooming, even with massive RAW or EXR files.
- **Modern Format Support:** Native support for standard web formats, high-dynamic-range imagery (EXR), vector graphics (SVG), and professional Camera RAW formats (.CR3, .ARW, .NEF, etc.).
- **Windows Integration:** Standard MSI installer provides a native "Open with Rivett" context menu and clean file associations.
- **Workflow-First UI:** A minimalist interface that stays out of your way, with an optional Info Panel for deep metadata inspection.

## 📥 Installation

The easiest way to use Rivett is to download the professional installer for your platform:

1.  Go to the [**Releases**](https://github.com/krets/rivett/releases) page.
2.  **Windows:** Download and run the `.msi` installer.
3.  **Linux:** Download the `.AppImage` (portable) or `.deb` (Debian/Ubuntu).
4.  **macOS:** Download the `.app` bundle.

## 📖 User Guide

Rivett is designed to be used primarily with your keyboard for maximum speed, but it fully supports mouse interaction for detailed inspection.

### Navigation
- **`Left` / `Right`**: Previous / Next image (resets zoom to "Fit").
- **`Shift + Arrow`**: Navigate while **preserving** your current zoom and pan position.
- **`PageUp` / `PageDown`**: Jump through your collection.

### Zoom & Pan
- **`Scroll Wheel`**: Zoom in/out at the cursor position.
- **`Left Click + Drag`**: Pan the image.
- **`F`**: Toggle "Fit to Window".
- **`Ctrl + 0`**: Zoom to Actual Size (1:1 pixels).
- **`Arrow Up / Down`**: Keyboard zoom.

### Vetting & Editing
- **`1` - `5`**: Set image rating.
- **`0`**: Clear rating.
- **`B`**: Toggle Bookmark.
- **`[` / `]`**: Rotate image 90° (saved automatically to the local database).
- **`Delete`**: Delete file (requires two-step confirmation).
- **`H`**: Hide image from current session.

### Tools
- **`I`**: Toggle Info Panel (Metadata/EXIF).
- **`Right Click`**: Open Context Menu for all options.
- **`Ctrl + Shift + R`**: Reset Session (clears filters and temporary state).
- **`Click on Error`**: If an image fails to load, click the error message to copy it for troubleshooting.

---

### 📷 Supported Formats
- **Standard:** PNG, JPEG, WebP, BMP, GIF
- **Professional:** OpenEXR (.exr), SVG
- **Camera RAW:** Canon (.CR2, .CR3), Sony (.ARW), Nikon (.NEF), Fujifilm (.RAF), Adobe Digital Negative (.DNG), and many more via LibRaw.

## License

MIT
