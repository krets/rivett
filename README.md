# Rivett

Rust Image Vetting Tool — a fast, keyboard-driven image viewer designed for quick sorting and vetting of large image collections.

## Features

- **Blazing Fast:** Built with Rust and OpenGL for smooth performance.
- **Keyboard-Driven:** Navigate and vet your images without touching the mouse.
- **Modern UI:** Clean, minimalist interface using `egui`.
- **Supported Formats:** PNG, JPEG, WebP, BMP, TIFF, GIF.

## Windows Installation (Beta)

To register Rivett in your Windows Explorer context menu and associate it with image files:

1.  Build the application:
    ```bash
    cargo build --release
    ```
2.  Run the installation script as **Administrator**:
    - Right-click `install.ps1` and select **Run with PowerShell**, or
    - Open an Administrative PowerShell terminal and run:
      ```powershell
      .\install.ps1
      ```
3.  The script will:
    - Register "Open with Rivett" in the right-click context menu.
    - Register Rivett as a handler for supported image types.
    - Offer to open the Windows Default Apps settings to set Rivett as your default viewer.

## Usage

Run Rivett from the command line:
```bash
rivett [path_to_image_or_folder]
```
Or simply use the "Open with Rivett" context menu in Windows Explorer.

### Keybindings
- `Left` / `Right`: Previous / Next image
- `Up` / `Down`: Zoom in / out
- `F`: Toggle Fullscreen
- `I`: Toggle Info Panel
- `Delete`: Vetting action (e.g., mark for deletion/move)
- `Escape`: Exit

## License

MIT
