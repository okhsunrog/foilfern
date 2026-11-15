# Linux downloads and releases

The first release series is 0.1.x. It is an early version; see the project status
and compatibility documents for the supported workflows and remaining gaps.

## Download and run

Linux x86_64 releases provide:

- `Foilfern-VERSION-x86_64.AppImage`: GUI plus the CLI and MCP server, with
  desktop libraries bundled. Requires glibc 2.35 or newer and a Linux desktop.
- `foilfern-VERSION-linux-x86_64.tar.gz`: native `bin/foilfern`,
  `bin/foilfern-cli`, `bin/foilfern-mcp`, license information and build identity.
- `SHA256SUMS`: verify the downloads with `sha256sum --check SHA256SUMS`.

For the AppImage, enable execution and start it:

```sh
chmod +x Foilfern-0.1.0-x86_64.AppImage
./Foilfern-0.1.0-x86_64.AppImage
```

If FUSE is unavailable, use `APPIMAGE_EXTRACT_AND_RUN=1` before the command.
On NixOS use `appimage-run`. Vulkan/OpenGL drivers come from the host; 3D needs
a supported graphics adapter. The headless CLI and MCP do not need a display.

The same AppImage can run either console program:

```sh
./Foilfern-0.1.0-x86_64.AppImage --run-cli --help
./Foilfern-0.1.0-x86_64.AppImage --run-mcp call server_info '{}'
```

For the native archive, unpack it and run a program from `bin/`. GUI use also
requires the distro's Fontconfig, FreeType, X11/Wayland and graphics libraries;
the AppImage is the simpler GUI download. No Rust toolchain, source checkout or
upstream reference fixtures are needed to run the installed programs.

## CI and release process

CI runs on pushes to master and pull requests, and can be started manually. It
checks formatting, full-workspace Clippy with warnings denied, and workspace
tests, using the pinned LibrePCB source/test resources and the upstream CLI.
Java 25 enables the optional Freerouting comparisons. Live supplier-network and
GPU-only ignored tests are not enabled automatically.

The Release workflow can be started manually from a branch to build and test
downloads without publishing. It first runs the same CI checks, then builds all
three release programs with HTTP enabled for MCP. Packaging tests the archive's
programs, the AppImage's console entry points and a real headless GUI screenshot.
The resulting downloads are retained as a workflow artifact for 14 days.

To publish, align the GUI/CLI/MCP package versions and push the corresponding
`vVERSION` tag, for example `v0.1.0`. The version guard rejects mismatches. After
all checks, packaging and checksum validation succeed, the workflow creates a
draft GitHub Release, attaches the checked files, then publishes it. Failed
builds never publish a release. Rerunning publication against an existing release
fails rather than replacing its downloads.

Action revisions and the AppImage packaging tool are pinned; the Rust channel is
stable. Both CI and packaging currently target Ubuntu 22.04 / Linux x86_64.
macOS and Windows downloads are outside this first release pipeline.
