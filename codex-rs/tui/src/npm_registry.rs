use serde::Deserialize;
use std::collections::HashMap;

#[cfg(not(debug_assertions))]
pub(crate) const PACKAGE_URL: &str = "https://registry.npmjs.org/@openai%2fcodex";
const PACKAGE_NAME: &str = "@openai/codex";

#[derive(Deserialize, Debug, Clone)]
pub(crate) struct NpmPackageInfo {
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    versions: HashMap<String, NpmPackageVersionInfo>,
}

#[derive(Deserialize, Debug, Clone)]
struct NpmPackageVersionInfo {
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: HashMap<String, String>,
    dist: Option<NpmPackageDist>,
}

#[derive(Deserialize, Debug, Clone)]
struct NpmPackageDist {
    tarball: Option<String>,
    integrity: Option<String>,
}

pub(crate) fn ensure_version_ready(
    package_info: &NpmPackageInfo,
    version: &str,
) -> anyhow::Result<()> {
    let version = version.trim();

    match package_info.dist_tags.get("latest").map(String::as_str) {
        Some(latest) if latest == version => {}
        Some(latest) => anyhow::bail!(
            "npm latest dist-tag points to {latest}, expected GitHub release {version}"
        ),
        None => anyhow::bail!("npm package is missing latest dist-tag"),
    }

    let version_info = version_info_with_dist(package_info, version)?;
    let expected_dependency_prefix = format!("{version}-");
    let mut codex_optional_dependencies = 0usize;
    for (dependency_name, dependency) in &version_info.optional_dependencies {
        let Some(dependency_version) = dependency
            .strip_prefix("npm:")
            .and_then(|dependency| dependency.strip_prefix(PACKAGE_NAME))
            .and_then(|dependency| dependency.strip_prefix('@'))
            .map(str::trim)
            .filter(|dependency| !dependency.is_empty())
        else {
            continue;
        };
        codex_optional_dependencies += 1;

        if !dependency_version.starts_with(&expected_dependency_prefix) {
            anyhow::bail!(
                "npm version {version} optional dependency {dependency_name} points to \
                 {dependency_version}, expected version starting with {expected_dependency_prefix}"
            );
        }

        if let Err(err) = version_info_with_dist(package_info, dependency_version) {
            anyhow::bail!(
                "npm version {version} optional dependency {dependency_name} points to \
                 unavailable version {dependency_version}: {err}"
            );
        }
    }

    if codex_optional_dependencies == 0 {
        anyhow::bail!("npm version {version} does not declare any codex optional dependencies");
    }

    Ok(())
}

fn version_info_with_dist<'a>(
    package_info: &'a NpmPackageInfo,
    version: &str,
) -> anyhow::Result<&'a NpmPackageVersionInfo> {
    let info = package_info
        .versions
        .get(version)
        .ok_or_else(|| anyhow::anyhow!("npm package version {version} is missing"))?;
    let Some(dist) = info.dist.as_ref() else {
        anyhow::bail!("npm package version {version} is missing dist metadata");
    };
    let has_tarball = dist
        .tarball
        .as_deref()
        .is_some_and(|tarball| !tarball.is_empty());
    if !has_tarball {
        anyhow::bail!("npm package version {version} is missing dist.tarball");
    }
    let has_integrity = dist
        .integrity
        .as_ref()
        .is_some_and(|integrity| !integrity.is_empty());
    if !has_integrity {
        anyhow::bail!("npm package version {version} is missing dist.integrity");
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version_json(version: &str) -> serde_json::Value {
        serde_json::json!({
            "dist": {
                "integrity": format!("sha512-{version}"),
                "tarball": format!("https://registry.npmjs.org/@openai/codex/-/codex-{version}.tgz"),
            }
        })
    }

    fn package_info<'a>(
        github_latest: &str,
        npm_latest: &str,
        optional_dependencies: impl IntoIterator<Item = (&'a str, &'a str)>,
        extra_versions: impl IntoIterator<Item = &'a str>,
    ) -> NpmPackageInfo {
        let mut versions = serde_json::Map::new();
        let mut root_version = version_json(github_latest);
        let optional_dependencies = optional_dependencies
            .into_iter()
            .map(|(name, dependency)| (name.to_string(), serde_json::json!(dependency)))
            .collect();
        root_version["optionalDependencies"] = serde_json::Value::Object(optional_dependencies);

        versions.insert(github_latest.to_string(), root_version);
        for version in extra_versions {
            versions.insert(version.to_string(), version_json(version));
        }

        serde_json::from_value(serde_json::json!({
            "dist-tags": { "latest": npm_latest },
            "versions": serde_json::Value::Object(versions),
        }))
        .expect("valid npm package metadata")
    }

    #[test]
    fn ready_version_requires_latest_dist_tag_and_optional_dependencies() {
        let latest = "1.2.3";
        let package_info = package_info(
            latest,
            latest,
            [
                (
                    "@openai/codex-darwin-arm64",
                    "npm:@openai/codex@1.2.3-darwin-arm64",
                ),
                (
                    "@openai/codex-linux-x64",
                    "npm:@openai/codex@1.2.3-linux-x64",
                ),
            ],
            ["1.2.3-darwin-arm64", "1.2.3-linux-x64"],
        );

        ensure_version_ready(&package_info, latest).expect("npm package is ready");
    }

    #[test]
    fn ready_version_rejects_stale_latest_dist_tag() {
        let package_info = package_info(
            "1.2.3",
            "1.2.2",
            [(
                "@openai/codex-darwin-arm64",
                "npm:@openai/codex@1.2.3-darwin-arm64",
            )],
            ["1.2.3-darwin-arm64"],
        );

        let err = ensure_version_ready(&package_info, "1.2.3")
            .expect_err("npm latest dist-tag must match GitHub latest");
        assert!(
            err.to_string().contains("latest dist-tag"),
            "error should name stale latest dist-tag: {err}"
        );
    }

    #[test]
    fn ready_version_rejects_missing_platform_version() {
        let latest = "1.2.3";
        let platform_version = "1.2.3-darwin-arm64";
        let package_info = package_info(
            latest,
            latest,
            [(
                "@openai/codex-darwin-arm64",
                "npm:@openai/codex@1.2.3-darwin-arm64",
            )],
            [],
        );

        let err = ensure_version_ready(&package_info, latest)
            .expect_err("platform tarball must be published");
        assert!(
            err.to_string().contains(platform_version),
            "error should name missing platform version: {err}"
        );
    }

    #[test]
    fn ready_version_rejects_dependency_for_different_release() {
        let latest = "1.2.3";
        let package_info = package_info(
            latest,
            latest,
            [(
                "@openai/codex-darwin-arm64",
                "npm:@openai/codex@1.2.2-darwin-arm64",
            )],
            ["1.2.2-darwin-arm64"],
        );

        let err = ensure_version_ready(&package_info, latest)
            .expect_err("platform dependency must match GitHub latest");
        assert!(
            err.to_string().contains("expected version starting"),
            "error should explain expected dependency version: {err}"
        );
    }
}
