# Arch distribution

CI reuses the verified native bundle to create a runtime tarball, then runs
`makepkg` as an unprivileged user. The resulting `omasheets-bin` package owns
`/usr/lib/omasheets`, two commands in `/usr/bin`, desktop/MIME entries and the
license. Standard Arch hooks refresh the desktop and MIME databases. There
are no root install scripts and no writes to a user's home directory.

The package omits the custom Setup/updater. Python modules live in a private
directory so a system Python minor-version upgrade does not strand them in an
old site-packages directory. Native binaries keep their verified provenance.

CI tests migration from the old install, Pacman installation, desktop checks,
an ordinary user's doctor and native window, the full service workflow,
upgrade to a higher pkgrel, removal and preservation of user files. Main CI
must pass before publication; another job downloads and installs the public
package without GitHub credentials.

## AUR publication

Publishing a GitHub release does not publish to the AUR. An AUR account and
its registered SSH key are required. No AUR credentials are stored here.

1. Download `omasheets-aur-recipe.tar.gz` from the intended passing release.
2. Inspect the extracted `PKGBUILD` and `.SRCINFO`. They pin the immutable
   release URL, version and SHA-256 of the runtime archive; no `SKIP` checksums
   or downloads of a moving `main` branch are used.
3. In the maintainer's `ssh://aur@aur.archlinux.org/omasheets-bin.git` checkout,
   copy those two files, commit and push. Do not push the binaries to the AUR.
4. Verify the public AUR entry before advertising `yay -S omasheets-bin` or
   Omarchy's **Install → AUR** route. Publish a new recipe for each intended
   update so AUR helpers see the newer version.

Development package versions use the source commit timestamp plus its short
SHA (`0.0.2.r<TIMESTAMP>.g<SHA>`). A later `0.1.0` sorts above these previews.
This channel remains a development preview and does not bypass the separate
signed production release gates. AUR source checksums verify downloaded bytes;
they are not a maintainer signature on a production release.

References: [Omarchy AUR picker](https://github.com/basecamp/omarchy/blob/master/bin/omarchy-pkg-aur-install),
[Arch package guidelines](https://wiki.archlinux.org/title/Arch_package_guidelines),
[AUR submission](https://wiki.archlinux.org/title/AUR_submission_guidelines).
