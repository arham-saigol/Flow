# Flow

Flow is a lightweight Windows dictation app built with Tauri 2, React, TypeScript,
Rust, and SQLite. Press Right Alt to record, press it again to finish, and Flow
transcribes, polishes, and pastes the result into the application where recording
began.

## Requirements

- Windows 10 or 11 with WebView2
- Node.js 20 or newer
- Stable Rust with the MSVC toolchain
- A Groq API key

## Development

```powershell
npm install
npm run tauri dev
```

The Groq API key is entered in Flow's Settings and stored as a generic credential
in Windows Credential Manager. Local history, dictionary entries, snippets, and
preferences are stored in Flow's application-data directory.

## Validation and release

```powershell
npm run typecheck
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

The release build produces an NSIS setup executable and an MSI package under
`src-tauri/target/release/bundle`.
