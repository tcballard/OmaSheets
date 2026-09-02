# Release procedure and trust roots

A published OmaSheets release is installable only when three independent
checks agree, and the user-local bootstrap refuses to open a bundle before
they do:

| Check | Root of trust | Who can forge it |
| --- | --- | --- |
| Detached minisign signature (`.minisig`) | Public key pinned at `release/signing-key.pub` in the validated plugin checkout; private key held offline by the maintainer | Only the holder of the offline private key |
| GitHub build-provenance attestation | GitHub's OIDC identity of the release workflow at the tagged commit (Sigstore) | Only a change to the workflow at that commit, visible in the tag |
| Release checksum, bundle manifest and executable provenance | The release channel itself | A compromised release credential |

The third row alone was the v0.0.2 model. The first two rows exist because a
release credential can replace an archive, its checksum and its self-reported
provenance together; neither the signing key nor the attestation identity is
reachable with that credential.

## Pinned build inputs

`.github/workflows/release.yml` pins every input of the build:

- actions by full commit SHA, with the version in a trailing comment;
- the container image by digest (`archlinux:base-devel@sha256:…`);
- the Arch package set by an Arch Linux Archive snapshot date
  (`ARCH_SNAPSHOT`), converged with `pacman -Syyuu` so the image's own
  package state cannot drift the result;
- the compiler's embedded paths through `-ffile-prefix-map`, and every
  timestamp through `SOURCE_DATE_EPOCH` set to the tagged commit's time.

`scripts/build_inputs.py` records the image digest, snapshot, installed
package versions, flags and workflow identity as `build-inputs.json`.
`scripts/build_native_bundle.py --build-inputs` embeds that record in the
bundle manifest under `build`, so every installation receipt carries it, and
writes the archive with fixed member order, ownership, permissions and
timestamps and a nameless, zero-mtime gzip header. Two builds of identical
executables therefore produce byte-identical archives with one SHA-256.

The workflow holds no signing secret. `scripts/check_release.py` fails the
release gate if any of these pins is missing or if `secrets.` appears in the
workflow.

## One-time key generation

The maintainer generates the release key on a machine that is not release
automation, with [minisign](https://jedisct1.github.io/minisign/) (Arch
package `minisign`):

```bash
minisign -G -p release/signing-key.pub -s ~/.minisign/omasheets-release.key
```

Commit `release/signing-key.pub`. The secret key stays outside the repository
and outside CI; `check_release.py --require-exact-version-tag` refuses to
release without the committed public key, and the bootstrap refuses to
download anything from a checkout that lacks it. Rotating the key is a normal
commit that replaces the file, followed by a new release signed with the new
key; older releases remain verifiable against the checkout that shipped them.

## Cutting a release

1. Bump the version everywhere `check_release.py` compares it and merge.
2. Tag the merge commit `v<version>` and push the tag. The workflow builds the
   bundle from the pinned inputs, attests it, and publishes the archive,
   `.sha256` and `build-inputs.json` as release assets. The release is not yet
   installable: the bootstrap requires the `.minisig` that only the maintainer
   can produce.
3. Verify the published archive before signing it. At minimum, confirm the
   attestation binds it to this repository's workflow at the tagged commit:

   ```bash
   gh attestation verify omasheets-native-<version>-linux-x86_64.tar.gz --repo tcballard/OmaSheets
   ```

   To verify reproducibility, rebuild locally from the recorded inputs and
   compare digests. The recipe is `build-inputs.json`: run the recorded image
   digest, point `/etc/pacman.d/mirrorlist` at the recorded snapshot, install
   the same package set with `pacman -Syyuu`, check out the tagged commit, and
   run `scripts/build_native_bundle.py` with the recorded `SOURCE_DATE_EPOCH`
   and `CXXFLAGS`. `sha256sum` of the rebuilt archive must equal the published
   `.sha256`.
4. Sign the archive offline with the default trusted comment, which binds the
   signature to the asset name:

   ```bash
   minisign -Sm omasheets-native-<version>-linux-x86_64.tar.gz -s ~/.minisign/omasheets-release.key
   python scripts/verify_release_signature.py omasheets-native-<version>-linux-x86_64.tar.gz
   ```

   `verify_release_signature.py` runs exactly the check the bootstrap runs,
   against the pinned key in the checkout.
5. Upload the signature as a release asset:

   ```bash
   gh release upload v<version> omasheets-native-<version>-linux-x86_64.tar.gz.minisig
   ```

From this point the bootstrap downloads the archive, verifies the signature
against the pinned key, verifies the release checksum, validates the bundle's
allow-listed contents, version, platform, architecture, exact source identity
and per-file hashes, and only then runs each executable's `--provenance`
check.

## What the bootstrap verifies

`omasheets.native_bundle.download_native_bundle` performs, in order:

1. the release boundary: checkout `HEAD` must be exactly the `v<version>` tag;
2. the pinned key: `release/signing-key.pub` must parse, before any network
   access;
3. download of the archive, then of its `.minisig` (bounded to 8 KiB);
4. minisign verification: key ID match, Ed25519 over the BLAKE2b-512 prehash
   of the archive, the global signature over the trusted comment, and the
   `file:` binding in that comment;
5. download and comparison of the `.sha256` checksum;
6. `install_native_bundle`'s existing content, identity and provenance checks.

The archive is deleted on any failure. The Ed25519 implementation in
`omasheets.release_signing` is standard-library only and is tested against the
RFC 8032 vectors so the bootstrap adds no runtime dependency.
