//! End-to-end validation of the classification pipeline against curated,
//! real-world GitHub repos (issue #21).
//!
//! Unlike the unit tests in `src/classify.rs`, these hit the *real* GitHub
//! API and real release-asset URLs, so every test here is `#[ignore]`d and
//! does not run as part of the default `cargo test --workspace`. Run them
//! explicitly, serially (to be polite to GitHub's unauthenticated rate
//! limit), before cutting a Phase 0 milestone release:
//!
//! ```bash
//! cargo test -p genjux-core --test e2e_real_repos -- --ignored --test-threads=1
//! ```
//!
//! Set `GENJUX_GITHUB_TOKEN` if you hit HTTP 403s running the full set
//! repeatedly in a short window.
//!
//! One test, `bottom_android_target_triple_is_not_misclassified_as_linux`,
//! is *expected to fail* when run this way: it documents a real classifier
//! bug (tracked in #51) found via this harness, kept as a failing
//! characterization test rather than silently special-cased away. Every
//! other test failing is a genuine regression.
//!
//! Each test looks up the specific asset(s) it wants to assert on, rather
//! than pinning every asset in the release — release contents drift over
//! time (new architectures get added, old ones dropped), and asserting on
//! the whole set would make these tests brittle for no real benefit. The
//! point is "does the pipeline get *these* representative assets right",
//! not "has this repo's release shape changed at all".
//!
//! Repos are chosen to cover the categories called out in #21's acceptance
//! criteria: clean per-platform naming, ambiguous/generic names that only
//! resolve via tier-2 keyword matching, missing/combined checksums, and a
//! case needing a `genjux.yaml` curator override.

use genjux_core::classify::{AssetKind, Platform};
use genjux_core::curator::{apply_overlay, load_overlay};
use genjux_core::package::{classify_release, InstallablePackage};
use genjux_core::source::github::GitHubProvider;
use genjux_core::source::{RepoRef, SourceProvider};
use genjux_core::verify::{parse_checksum_manifest, parse_single_checksum_file};

/// Fetches the latest release of a real repo and runs it through the same
/// `classify_release` pipeline production code uses.
async fn latest_packages(owner: &str, repo: &str) -> Vec<InstallablePackage> {
    let provider = GitHubProvider::from_env();
    let repo_ref = RepoRef::new("github", owner, repo);
    let release = provider
        .latest_release(&repo_ref)
        .await
        .unwrap_or_else(|e| panic!("fetching {owner}/{repo} releases failed: {e}"))
        .unwrap_or_else(|| panic!("{owner}/{repo} has no releases"));
    classify_release(&release)
}

/// Finds the one asset whose name contains every given substring *and* ends
/// with `ends_with` (pass `""` when there's no useful suffix anchor, e.g.
/// for jq's extensionless binaries). The suffix anchor matters: several of
/// these repos publish a `<asset>.sha256`/`.sha256sum`/`.zsync` sidecar file
/// right next to the real asset, whose name is a superset of the asset's
/// own name — a plain substring-only match ends up matching both. Panics
/// with the full asset list if there's no match (or more than one) — a
/// curated repo changing its naming convention entirely is exactly the
/// kind of drift this harness should surface loudly, not swallow.
fn find_one<'a>(
    packages: &'a [InstallablePackage],
    name_contains: &[&str],
    ends_with: &str,
) -> &'a InstallablePackage {
    let matches: Vec<&InstallablePackage> = packages
        .iter()
        .filter(|p| {
            name_contains
                .iter()
                .all(|needle| p.asset_name.contains(needle))
                && p.asset_name.ends_with(ends_with)
        })
        .collect();
    match matches.as_slice() {
        [one] => one,
        _ => panic!(
            "expected exactly one asset containing {name_contains:?} and ending with {ends_with:?}, found {}: {:?}",
            matches.len(),
            packages.iter().map(|p| &p.asset_name).collect::<Vec<_>>()
        ),
    }
}

async fn fetch_text(url: &str) -> String {
    reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("GET {url} failed: {e}"))
        .text()
        .await
        .unwrap_or_else(|e| panic!("reading body of {url} failed: {e}"))
}

// --- Clean, unambiguous per-platform naming (tier 1 extension + target triple arch) ---

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn ripgrep_classifies_cleanly_and_per_asset_sha256_matches_manifest_format() {
    let packages = latest_packages("BurntSushi", "ripgrep").await;

    let macos = find_one(&packages, &["aarch64-apple-darwin"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));
    assert_eq!(
        macos.classification.arch,
        Some(genjux_core::classify::Arch::Arm64)
    );

    let windows = find_one(&packages, &["x86_64-pc-windows-msvc"], ".zip");
    assert_eq!(windows.classification.platform, Some(Platform::Windows));

    // Per-asset `.sha256` sidecar uses the "<hex>  <filename>" manifest
    // format (matching filename verbatim) — exercises the primary branch
    // of parse_single_checksum_file, not just its fallback.
    let sidecar_url = format!("{}.sha256", macos.download_url);
    let sidecar_contents = fetch_text(&sidecar_url).await;
    let digest = parse_single_checksum_file(&sidecar_contents, &macos.asset_name);
    assert!(
        digest.is_some_and(|d| d.len() == 64),
        "expected a 64-char hex digest from {sidecar_url}"
    );
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn bat_separates_linux_deb_from_macos_tarball_without_bleed() {
    let packages = latest_packages("sharkdp", "bat").await;

    let macos = find_one(&packages, &["x86_64-apple-darwin"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));

    let deb = find_one(&packages, &["bat_"], "amd64.deb");
    assert_eq!(deb.classification.platform, Some(Platform::Linux));
    assert_eq!(deb.classification.kind, Some(AssetKind::Deb));
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn starship_msi_and_darwin_tarball_classify_to_different_platforms() {
    let packages = latest_packages("starship", "starship").await;

    let windows_msi = find_one(&packages, &["x86_64-pc-windows-msvc"], ".msi");
    assert_eq!(windows_msi.classification.platform, Some(Platform::Windows));
    assert_eq!(windows_msi.classification.kind, Some(AssetKind::Msi));

    let macos = find_one(&packages, &["x86_64-apple-darwin"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn neovim_macos_windows_and_appimage_all_classify_distinctly() {
    let packages = latest_packages("neovim", "neovim").await;

    let macos = find_one(&packages, &["nvim-macos-arm64"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));

    let windows = find_one(&packages, &["nvim-win64"], ".msi");
    assert_eq!(windows.classification.platform, Some(Platform::Windows));
    assert_eq!(windows.classification.kind, Some(AssetKind::Msi));

    let appimage = find_one(&packages, &["nvim-linux-x86_64"], ".appimage");
    assert_eq!(appimage.classification.platform, Some(Platform::Linux));
    assert_eq!(appimage.classification.kind, Some(AssetKind::AppImage));

    // neovim publishes no checksums at all for its releases — confirm we
    // simply have nothing to look up rather than crashing/guessing. This
    // is the "missing checksums" category from #21's acceptance criteria.
    assert!(
        !packages
            .iter()
            .any(|p| p.asset_name.to_lowercase().contains("sha256")
                || p.asset_name.to_lowercase().contains("checksum")),
        "expected no checksum manifest asset in a neovim release"
    );
}

// --- Ambiguous/generic naming, only resolved via tier-2 keyword matching ---

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn fzf_darwin_underscore_naming_resolves_via_keyword_tier_and_checksums_manifest_parses() {
    let packages = latest_packages("junegunn", "fzf").await;

    // No target triple, no OS-specific extension — "fzf-X.Y.Z-darwin_arm64.tar.gz"
    // only resolves because tier 2 recognizes "darwin" plus an arch token.
    let macos = find_one(&packages, &["darwin_arm64"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));
    assert_eq!(
        macos.classification.arch,
        Some(genjux_core::classify::Arch::Arm64)
    );
    assert_eq!(
        macos.classification.kind,
        Some(AssetKind::Archive),
        "generic .tar.gz resolved only by tier 2 should be tagged as a generic Archive"
    );

    // fzf also ships an explicit "android_arm64" asset — must not bleed
    // into Linux (it contains no "linux" substring, so this one isn't
    // exercising the ordering bug found in bottom's target triple below,
    // just confirming the plain keyword match works).
    let android = find_one(&packages, &["android_arm64"], ".tar.gz");
    assert_eq!(android.classification.platform, Some(Platform::Android));

    // Combined checksums manifest covering every asset in one file — the
    // "missing per-asset checksums" category from #21's acceptance
    // criteria.
    let checksums_asset = packages
        .iter()
        .find(|p| p.asset_name.contains("checksums.txt"))
        .expect("fzf release should publish a combined checksums.txt");
    let manifest = fetch_text(&checksums_asset.download_url).await;
    let digest = parse_checksum_manifest(&manifest, &macos.asset_name);
    assert!(
        digest.is_some_and(|d| d.len() == 64),
        "expected to find {} in the combined checksums manifest",
        macos.asset_name
    );
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn restic_darwin_bz2_resolves_platform_via_keyword_with_no_recognized_kind() {
    let packages = latest_packages("restic", "restic").await;

    // ".bz2" isn't one of our recognized archive/installer extensions, so
    // tier 1 leaves this fully unclassified; tier 2's "darwin" keyword is
    // the *only* thing that resolves the platform. `kind` correctly stays
    // `None` — we recognize the platform without pretending to know an
    // install mechanism for a bare compressed binary.
    let macos = find_one(&packages, &["restic_", "darwin_amd64"], ".bz2");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));
    assert_eq!(macos.classification.kind, None);
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn jq_extensionless_binaries_resolve_via_keyword_and_combined_manifest_parses() {
    let packages = latest_packages("jqlang", "jq").await;

    // "jq-macos-arm64" has no extension whatsoever — tier 1 can't help at
    // all; only the "macos" keyword in tier 2 resolves it.
    let macos = find_one(&packages, &["jq-macos-arm64"], "");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));
    assert_eq!(macos.classification.kind, None);

    // ".exe" is a clean tier-1 match, right in the same release.
    let windows = find_one(&packages, &["jq-windows-amd64"], ".exe");
    assert_eq!(windows.classification.platform, Some(Platform::Windows));
    assert_eq!(windows.classification.kind, Some(AssetKind::Exe));

    let checksums_asset = packages
        .iter()
        .find(|p| p.asset_name == "sha256sum.txt")
        .expect("jq release should publish a combined sha256sum.txt");
    let manifest = fetch_text(&checksums_asset.download_url).await;
    let digest = parse_checksum_manifest(&manifest, &macos.asset_name);
    assert!(
        digest.is_some_and(|d| d.len() == 64),
        "expected to find {} in jq's combined sha256sum.txt",
        macos.asset_name
    );
}

// --- Auxiliary (non-installable) files must not be misclassified as platforms ---

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn zellij_checksum_sidecars_never_get_an_installable_kind() {
    let packages = latest_packages("zellij-org", "zellij").await;

    let macos = find_one(&packages, &["zellij-x86_64-apple-darwin"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));

    // zellij's `.sha256sum` sidecar is literally named after the asset it
    // checksums (e.g. "zellij-x86_64-apple-darwin.sha256sum"), so tier 2's
    // "darwin" keyword match correctly fires on the sidecar's name too —
    // that's not a bug, the platform guess is even accurate. What *must*
    // never happen is the sidecar being tagged with an installable `kind`
    // (`.sha256sum` isn't a recognized archive/installer extension, so
    // kind stays `None`) — that's the actual invariant protecting install
    // orchestration from ever trying to "install" a checksum file.
    let sidecar = find_one(&packages, &["zellij-x86_64-apple-darwin.sha256sum"], "");
    assert_eq!(sidecar.classification.kind, None);

    // Real-world quirk: zellij's sha256sum sidecar contents reference the
    // original build path (e.g. "target/x86_64-apple-darwin/release/
    // zellij"), not the release asset's own filename — so the primary
    // "<hex>  <filename>" manifest-format match in
    // parse_single_checksum_file can't succeed, and it must fall back to
    // "first token on the first line, if it looks like a sha256 hex
    // digest". This is exactly the fallback parse_single_checksum_file was
    // built for (see core/src/verify.rs) — assert it actually works
    // against real content, not just a synthetic unit-test fixture.
    let sidecar_contents = fetch_text(&sidecar.download_url).await;
    let digest = parse_single_checksum_file(&sidecar_contents, &macos.asset_name);
    assert!(
        digest.is_some_and(|d| d.len() == 64),
        "expected the fallback first-hex-token parse to succeed against zellij's sidecar format"
    );
}

// --- Source-only archives must never be guessed at ---

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn helix_source_archive_stays_unclassified_while_platform_builds_do_not() {
    let packages = latest_packages("helix-editor", "helix").await;

    let macos = find_one(&packages, &["x86_64-macos"], ".tar.xz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));

    let windows = find_one(&packages, &["x86_64-windows"], ".zip");
    assert_eq!(windows.classification.platform, Some(Platform::Windows));

    // "helix-X.Y.Z-source.tar.xz" has no OS keyword and no OS-specific
    // extension — it must come back unclassified, never silently guessed
    // as belonging to some platform.
    let source = find_one(&packages, &["-source"], ".tar.xz");
    assert_eq!(source.classification.platform, None);
}

// --- Known classification bug found via this harness (tracked separately, not special-cased here) ---

#[tokio::test]
#[ignore = "known bug: Android target triples containing \"linux\" are misclassified as \
            Linux (keyword-check ordering in classify::refine_by_keywords); tracked in \
            https://github.com/PetrGuan/Genjux-Store/issues/51 — un-ignore once fixed"]
async fn bottom_android_target_triple_is_not_misclassified_as_linux() {
    let packages = latest_packages("ClementTsang", "bottom").await;

    // "bottom_aarch64-linux-android.tar.gz" is a real Rust target triple
    // for Android, but refine_by_keywords checks `contains("linux")`
    // before `contains("android")`, so it currently (incorrectly) matches
    // Linux first. Filed as #51 rather than patched inline here, per #21's
    // acceptance criteria: a classification bug found by this validation
    // harness becomes a tracked follow-up issue, not a silent special
    // case bolted onto the harness or the classifier.
    let android = find_one(&packages, &["aarch64-linux-android"], ".tar.gz");
    assert_eq!(android.classification.platform, Some(Platform::Android));
}

#[tokio::test]
#[ignore = "hits the real GitHub API/CDN; run explicitly, see module docs"]
async fn bottom_macos_linux_and_windows_native_assets_classify_correctly() {
    let packages = latest_packages("ClementTsang", "bottom").await;

    let macos = find_one(&packages, &["bottom_aarch64-apple-darwin"], ".tar.gz");
    assert_eq!(macos.classification.platform, Some(Platform::MacOS));

    let deb = find_one(&packages, &["bottom_"], "amd64.deb");
    assert_eq!(deb.classification.platform, Some(Platform::Linux));
    assert_eq!(deb.classification.kind, Some(AssetKind::Deb));

    let windows_installer = find_one(&packages, &["bottom_aarch64_installer"], ".msi");
    assert_eq!(
        windows_installer.classification.platform,
        Some(Platform::Windows)
    );
    assert_eq!(windows_installer.classification.kind, Some(AssetKind::Msi));
}

// --- genjux.yaml curator override (tier 4), demonstrated end-to-end ---
//
// A real repo that *needs* a genjux.yaml override to fix a wrong/missing
// classification wasn't identified among the curated set above (the ones
// tiers 1-3 can't handle correctly all turned out to be genuine classifier
// bugs worth fixing upstream in classify.rs, not override material — see
// bottom's Android/Linux case above). So this demonstrates the override
// mechanism against a realistic *constructed* scenario instead: an asset
// with no OS keyword and no recognized extension at all, which tiers 1-3
// correctly leave unclassified, resolved only by an explicit genjux.yaml
// entry — exactly the scenario tier 4 exists for (see core/src/curator.rs
// and core/docs/genjux-yaml.md).

#[test]
fn genjux_yaml_override_resolves_an_asset_tiers_1_to_3_cannot() {
    use genjux_core::source::{Release, ReleaseAsset};

    let release = Release {
        tag: "v1.0.0".to_string(),
        assets: vec![ReleaseAsset {
            name: "MyApp-universal.zip".to_string(),
            size_bytes: 12_345,
            download_url: "https://example.invalid/MyApp-universal.zip".to_string(),
            content_type: None,
        }],
    };

    let mut packages = classify_release(&release);
    assert_eq!(
        packages[0].classification.platform, None,
        "a generic archive with no OS keyword must stay unclassified through tiers 1-3"
    );

    let overlay = load_overlay(
        r#"
assets:
  MyApp-universal.zip:
    platform: macos
    arch: arm64
    min_os_version: "12.0"
"#,
    )
    .expect("valid genjux.yaml overlay should parse");

    apply_overlay(&overlay, &mut packages);

    assert_eq!(packages[0].classification.platform, Some(Platform::MacOS));
    assert_eq!(packages[0].min_os_version.as_deref(), Some("12.0"));
}
