# Development

[Back to Patchlook](../README.md)

## Prerequisites

All platforms need Rust 1.95+ and Git on `PATH`. Packaging and packaging tests also need Python 3.11+.

| Platform | Build dependencies |
| --- | --- |
| macOS | macOS 15+, Xcode, and its command-line tools. `xcode-select -p` must point to a valid Xcode developer directory. |
| Windows | The MSVC Rust toolchain, Visual Studio 2022 with **Desktop development with C++** and the Windows SDK, CMake, and Git for Windows. |
| Linux | A C/C++ toolchain and the native libraries below. Running the app needs X11 or Wayland and a Vulkan-capable graphics driver. |

On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install -y gcc g++ clang cmake pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libssl-dev libzstd-dev libvulkan1
```

## Run and test

```sh
cargo run --locked
cargo run --locked -- /path/to/repository
```

Quote repository paths that contain spaces.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Use `python` instead of `python3` on Windows. Git tests use isolated temporary repositories. The test suites cover repository reads, preferences and localization, tree and image behavior, diff layout and scrolling, and packaging failure paths.

## Package a local build

```sh
cargo build --locked --release
python3 scripts/package.py
```

The script packages an existing binary and writes its output to `dist/`:

| Platform | Output |
| --- | --- |
| macOS | `Patchlook.app`, ZIP, and a DMG with an Applications shortcut |
| Windows | ZIP containing `patchlook.exe` and the README |
| Linux | tar.gz containing `patchlook` and the README |

The macOS icon is generated from [the app icon source](../assets/app-icon/patchlook-v2.png). Local macOS packages use ad-hoc signing by default. Linux builds link against system libraries and are not universal distribution packages.

For a local signed and notarized macOS build, install a Developer ID Application certificate and store notarization credentials with `xcrun notarytool store-credentials`, then run:

```sh
python3 scripts/package.py \
  --sign-identity "Developer ID Application: Name (TEAMID)" \
  --notary-profile patchlook-notary
```

## Releases

The [Desktop workflow](../.github/workflows/desktop.yml) checks and packages all three platforms on `main` and pull requests. Ordinary builds do not read release credentials.

To prepare a release:

1. Set the version in `Cargo.toml` and update `Cargo.lock`.
2. Commit and push the changes.
3. Push a matching version tag, such as `v0.1.0`.
4. Review the generated Release draft and its packages before publishing it.

For version tags, the macOS job imports the signing certificate into a temporary keychain, signs with Hardened Runtime and a secure timestamp, submits the DMG to Apple, staples and validates notarization tickets, and checks Gatekeeper acceptance. The ZIP contains the stapled app. A Release draft is created or updated only after all platforms pass; published release assets are not replaced.

Configure these repository secrets under **Settings → Secrets and variables → Actions**:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_BASE64` | Base64 of the Developer ID Application `.p12`, including its private key |
| `MACOS_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `MACOS_SIGNING_IDENTITY` | Full certificate name: `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | Apple Account email |
| `APPLE_TEAM_ID` | Paid developer team ID |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password for notarization |

Keep certificates, private keys, and passwords out of the repository. Missing credentials or a failed signing, notarization, or ticket check stops the release job.

### Check a notarization timeout

The workflow waits up to 30 minutes for Apple. A timeout stops CI but does not cancel Apple's processing.

In **Actions → Desktop → Run workflow**, select `main` and enter the existing submission ID in `notary_submission_id`. This queries Apple's status and, when processing has finished, its log. It does not rebuild or upload the app. Results appear in the run's logs and Summary. Leave the input empty to run normal build checks.

Check the existing submission before rerunning a failed release. An `Accepted` response confirms that Apple has accepted that submission; the release still needs successful packaging, ticket verification, and artifact upload.

## GPUI Kit patches

Patchlook uses the [GPUI Kit fork](https://github.com/worldtheater/gpui-kit/tree/codex/diff-tool-patches), on the `codex/diff-tool-patches` branch. [Cargo.toml](../Cargo.toml) pins its Git revision through `[patch.crates-io]`; [Cargo.lock](../Cargo.lock) is checked in. Cargo fetches the source automatically, so normal builds need neither a submodule nor a sibling checkout.

The patch series starts from the GPUI Kit 0.8.0 source commit `1676826d9d49b90f4d7b6808b69924f58796f6f1`. Its current tip is `e2a084aedbd36212545cf6242cc1c58f4b834562`.

| Commit | Purpose |
| --- | --- |
| `697e0e7e` | Apply touchpad release velocity only when an edge is already stretched. |
| `085bc94c` | Consume fractional scrolling within bounds to avoid accidental bounce. |
| `634f5b54` | Keep diff panes equally sized, synchronize horizontal scrolling, and constrain text selection to the visible pane. |
| `dcc55bcc` | Add collapsible diff sections and keep change markers aligned with wrapped lines. |
| `9697b217` | Count contiguous changed blocks independently of folding and layout. |
| `e2a084ae` | Localize diff controls in nine languages and rebuild when locale resources change. |

When updating GPUI Kit:

1. Start a new patch branch from the target upstream revision.
2. Port only the patches still needed. Upstream commits `e692bcb7` and `cf5b8639` address the two scrolling fixes; check their behavior before carrying those patches forward.
3. Push the patch branch, then update the pinned revision and lockfile in Patchlook.
4. Run the checks above and a release build. Keep the consumer regressions in [diff_layout.rs](../tests/diff_layout.rs) and [scroll_bounce.rs](../tests/scroll_bounce.rs).

For local work on the fork, temporarily replace the `gpui-kit` Git patch with a path to its `crates/kit` directory. Restore the Git revision and lockfile before committing; do not commit machine-specific paths.
