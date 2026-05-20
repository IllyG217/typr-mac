# Typr — macOS

This is the macOS version of Typr. It shares the same Rust/TypeScript codebase as the Windows version with a few platform-specific differences.

---

## Differences from the Windows version

| Area | Windows (`typr/`) | macOS (`typr-mac/`) |
|---|---|---|
| Paste simulation | `enigo` → Ctrl+V | `osascript` → Cmd+V |
| API key storage | Windows Credential Manager | macOS Keychain |
| Mouse event suppression | `rdev::grab` (WH_MOUSE_LL hook) | `rdev::grab` (CGEventTap) |
| Installer format | NSIS `.exe` / MSI | `.dmg` disk image |
| `windows_subsystem` | Present (hides console) | Removed (not applicable) |
| Whisper binary | `.exe` + `.dll` files | Single statically-linked binary |

---

## Building from source (requires a Mac)

> **You cannot build a macOS `.dmg` from Windows.** You need either a Mac or a GitHub Actions runner.

### On a Mac:

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Add Apple Silicon and Intel targets
rustup target add aarch64-apple-darwin x86_64-apple-darwin

# 3. Install Node.js (https://nodejs.org) then:
npm install

# 4. Build the whisper-cpp sidecar binary and name it correctly
#    Tauri looks for: binaries/whisper-cpp-aarch64-apple-darwin  (Apple Silicon)
#                     binaries/whisper-cpp-x86_64-apple-darwin   (Intel)
#
#    Quick build for the current machine:
TARGET=$(rustc -vV | awk '/host:/ { print $2 }')
git clone --depth 1 https://github.com/ggerganov/whisper.cpp /tmp/whisper-src
cd /tmp/whisper-src && cmake -B build && cmake --build build --config Release -j$(sysctl -n hw.logicalcpu)
mkdir -p src-tauri/binaries
cp /tmp/whisper-src/build/bin/whisper-cli src-tauri/binaries/whisper-cpp-$TARGET
chmod +x src-tauri/binaries/whisper-cpp-$TARGET

# 5. Build the app
npm run tauri build
# Output: src-tauri/target/release/bundle/dmg/Typr_0.1.4_x64.dmg
```

### Via GitHub Actions (no Mac needed):

1. Push this `typr-mac/` folder to a GitHub repository
2. The workflow at `.github/workflows/build-mac.yml` runs automatically
3. It builds for **both Apple Silicon and Intel** and uploads the `.dmg` as a GitHub Actions artifact
4. Download the artifact from the Actions tab → the `.dmg` is ready to install

---

## macOS permissions required

On first launch, macOS will ask for:

| Permission | Why |
|---|---|
| **Microphone** | Audio recording |
| **Accessibility** | Global hotkey, mouse button grab, Cmd+V paste via System Events |

To grant Accessibility access: **System Settings → Privacy & Security → Accessibility → enable Typr**

If Typr can't paste text or the hotkey doesn't work, Accessibility permission is the first thing to check.

---

## Installing the .dmg

1. Open the `.dmg` file
2. Drag **Typr.app** into your **Applications** folder
3. Launch Typr from Applications
4. macOS may show "unidentified developer" — right-click the app → Open → Open anyway
5. Grant Microphone and Accessibility permissions when prompted

---

## Version history

| Version | Notes |
|---|---|
| 0.1.4 | Initial macOS port — grab-based event suppression, phantom release fix, correct button labels |
