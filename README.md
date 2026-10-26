<p align="center">
  <img src="https://raw.githubusercontent.com/worldtheater/patchlook/main/assets/app-icon/patchlook-v2.png" alt="Patchlook logo" width="112" height="112">
</p>

<h1 align="center">Patchlook</h1>

<p align="center">A native, read-only Git viewer for macOS, Windows, and Linux.</p>

<p align="center">
  <a href="https://github.com/worldtheater/patchlook/releases">Releases</a> ·
  <a href="#build-from-source">Build from source</a> ·
  <a href="https://github.com/worldtheater/patchlook/issues">Report an issue</a>
</p>

Inspect local changes, explore commit history, and compare file versions without changing your working tree, staging area, or branches.

## Features

- **Review changes** — inspect staged, unstaged, and untracked files, with added and deleted line counts.
- **Explore history** — browse commits and local or remote-tracking branches, with full commit messages and a file tree.
- **Compare code** — switch between side-by-side and inline diffs, with syntax highlighting and changes highlighted within each line.
- **Stay focused** — collapse unchanged sections, wrap long lines, or scroll both versions together.
- **Compare images** — view versions side by side, fit them to the window, or zoom and pan to inspect details.
- **Make it yours** — light, dark, and system themes, plus nine interface languages.

## Install

Browse [Releases](https://github.com/worldtheater/patchlook/releases) for published builds, or [build from source](#build-from-source).

| Platform | Requirements | Installation |
| --- | --- | --- |
| macOS | macOS 15+, Apple Silicon | Open the DMG and drag **Patchlook** into **Applications**. |
| Windows | Windows 10+, x64 | Extract the ZIP and run `patchlook.exe`. |
| Linux | x86_64, X11 or Wayland, Vulkan-capable graphics | Extract the tar.gz and run `./patchlook`. |

[Git](https://git-scm.com/downloads) must be installed and available on `PATH`. macOS release builds are signed and notarized. Linux packages are built on Ubuntu 24.04 and require compatible system libraries.

## Getting started

1. Choose **Open repository** and select a local Git repository.
2. Use **Changes** to review your working tree and staging area, or **History** to inspect commits.
3. Use **Repos** to switch repositories or browse another branch.

Refresh after making changes in another tool. Branch browsing uses local Git references; fetch updates with your usual Git client.

Text diffs require UTF-8. Supported images are shown as previews; other binary files are listed without a content diff.

## Keyboard shortcuts

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Open repository | `⌘ O` | `Ctrl O` |
| Refresh | `⌘ R` / `F5` | `Ctrl R` / `F5` |
| Copy selection | `⌘ C` | `Ctrl C` |
| Select all in the current diff pane | `⌘ A` | `Ctrl A` |
| Quit | `⌘ Q` | `Ctrl Q` |

## Build from source

Install Rust 1.95+, Git, and the [platform build dependencies](https://github.com/worldtheater/patchlook/blob/main/docs/development.md#prerequisites), then run:

```sh
git clone https://github.com/worldtheater/patchlook.git
cd patchlook
cargo run --locked
```

See the [development guide](https://github.com/worldtheater/patchlook/blob/main/docs/development.md) for testing, packaging, releases, and GPUI Kit patches.

Built with [Rust](https://www.rust-lang.org/) and [GPUI Kit](https://github.com/longbridge/gpui-kit).
