//! Administrator-configured View-capability grants for stemd applications.
//!
//! A grant fixes both the application identity and its complete argument vector.
//! Runtime callers can therefore only select an existing grant; they cannot add
//! paths or arguments that were not present in the administrator's configuration.

use serde::Deserialize;

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_APPLICATIONS: usize = 32;
const MAX_VIEWS_PER_APPLICATION: usize = 16;
const MAX_ARGUMENTS: usize = 256;
const MAX_APPLICATION_ID_BYTES: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationGrant {
    pub id: String,
    pub argv: Vec<String>,
    pub view: Vec<ViewGrant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewGrant {
    pub root: String,
    pub target: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityConfig {
    #[serde(default)]
    application: Vec<ApplicationEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationEntry {
    id: String,
    argv: Vec<String>,
    view: Vec<ViewEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewEntry {
    root: String,
    target: u32,
}

/// Parse administrator-owned capability configuration.
///
/// The TOML document has a `[[application]]` entry for every trusted app and a
/// `[[application.view]]` entry for every native View to transfer. Unknown
/// fields, ambiguous grants, and unsafe paths are rejected.
pub fn parse(text: &str) -> Result<Vec<ApplicationGrant>, &'static str> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err("capability configuration exceeds 64 KiB");
    }

    let config: CapabilityConfig =
        toml::from_str(text).map_err(|_| "invalid capability configuration")?;
    if config.application.len() > MAX_APPLICATIONS {
        return Err("capability configuration has too many applications");
    }

    let mut grants = Vec::with_capacity(config.application.len());
    let mut application_ids = Vec::with_capacity(config.application.len());
    let mut argument_count = 0usize;

    for application in config.application {
        validate_application_id(&application.id)?;
        if application_ids.contains(&application.id) {
            return Err("duplicate application ID");
        }

        if application.argv.is_empty() {
            return Err("application argv is empty");
        }
        argument_count = argument_count
            .checked_add(application.argv.len())
            .filter(|&count| count <= MAX_ARGUMENTS)
            .ok_or("capability configuration has too many arguments")?;
        for (index, argument) in application.argv.iter().enumerate() {
            if argument.as_bytes().contains(&0) {
                return Err("argument contains NUL");
            }
            if index == 0 {
                validate_absolute_path(argument)?;
            }
        }

        if application.view.is_empty() {
            return Err("application has no View grants");
        }
        if application.view.len() > MAX_VIEWS_PER_APPLICATION {
            return Err("application has too many View grants");
        }

        let mut targets = Vec::with_capacity(application.view.len());
        let mut views = Vec::with_capacity(application.view.len());
        for view in application.view {
            validate_absolute_path(&view.root)?;
            if !(3..=255).contains(&view.target) {
                return Err("View target must be between 3 and 255");
            }
            if targets.contains(&view.target) {
                return Err("duplicate target for application");
            }
            targets.push(view.target);
            views.push(ViewGrant {
                root: view.root,
                target: view.target,
            });
        }

        application_ids.push(application.id.clone());
        grants.push(ApplicationGrant {
            id: application.id,
            argv: application.argv,
            view: views,
        });
    }

    Ok(grants)
}

/// Select a configured grant using the application's exact identity and argv.
///
/// An unknown application returns `Ok(None)`. If an application ID is present
/// but any argument differs, the request is rejected so an unconfigured argv
/// cannot fall back to another application or an implicit capability path.
pub fn find_grant<'a>(
    grants: &'a [ApplicationGrant],
    id: &str,
    argv: &[String],
) -> Result<Option<&'a ApplicationGrant>, &'static str> {
    let mut found = None;
    for grant in grants {
        if grant.id == id {
            if found.is_some() {
                return Err("duplicate application ID");
            }
            found = Some(grant);
        }
    }

    match found {
        None => Ok(None),
        Some(grant) if grant.argv.as_slice() == argv => Ok(Some(grant)),
        Some(_) => Err("application argv does not match its capability grant"),
    }
}

fn validate_application_id(id: &str) -> Result<(), &'static str> {
    // Keep this grammar aligned with the native app descriptor validation in
    // scarlet-apps::parse_app. The app ID is the descriptor filename stem; it
    // is matched exactly rather than normalized as a path.
    if id.is_empty()
        || id.len() > MAX_APPLICATION_ID_BYTES
        || id == "."
        || id == ".."
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err("invalid application ID");
    }
    Ok(())
}

fn validate_absolute_path(path: &str) -> Result<(), &'static str> {
    if !path.starts_with('/') {
        return Err("path must be absolute");
    }
    if path.as_bytes().contains(&0) {
        return Err("path contains NUL");
    }
    if path == "/" {
        return Ok(());
    }

    let components = &path[1..];
    if components
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err("path contains an invalid or traversal component");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRUSTED: &str = r#"
        [[application]]
        id = "org.scarlet.Android"
        argv = ["/bin/android", "--view-handle", "3"]

        [[application.view]]
        root = "/"
        target = 3
    "#;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn trusted_match_returns_the_configured_grant() {
        let grants = parse(TRUSTED).unwrap();
        let argv = strings(&["/bin/android", "--view-handle", "3"]);
        let grant = find_grant(&grants, "org.scarlet.Android", &argv)
            .unwrap()
            .unwrap();

        assert_eq!(
            grant.view,
            vec![ViewGrant {
                root: "/".into(),
                target: 3
            }]
        );
    }

    #[test]
    fn argv_override_is_rejected_for_a_configured_application() {
        let grants = parse(TRUSTED).unwrap();
        let argv = strings(&["/bin/android", "--view-handle", "4"]);

        assert!(find_grant(&grants, "org.scarlet.Android", &argv).is_err());
    }

    #[test]
    fn unregistered_application_has_no_grant() {
        let grants = parse(TRUSTED).unwrap();
        let argv = strings(&["/bin/other", "--view-handle", "3"]);

        assert_eq!(find_grant(&grants, "org.scarlet.Other", &argv), Ok(None));
    }

    #[test]
    fn application_id_grammar_matches_catalog_and_matching_is_exact() {
        for id in [
            "org.scarlet.Android",
            ".leading",
            "trailing.",
            "double..dot",
            "-leading",
            "trailing_",
        ] {
            assert!(validate_application_id(id).is_ok(), "{id}");
        }
        for id in [
            "",
            ".",
            "..",
            "org/scarlet",
            "org scarlet",
            "org.scarlet.\u{e9}",
        ] {
            assert!(validate_application_id(id).is_err(), "{id}");
        }

        let config = TRUSTED.replace("org.scarlet.Android", "double..dot");
        let grants = parse(&config).unwrap();
        let argv = strings(&["/bin/android", "--view-handle", "3"]);
        assert!(find_grant(&grants, "double..dot", &argv).unwrap().is_some());
        assert_eq!(find_grant(&grants, "double.dot", &argv), Ok(None));
    }

    #[test]
    fn path_traversal_and_invalid_paths_are_rejected() {
        let traversal = TRUSTED.replace("root = \"/\"", "root = \"/tmp/../etc\"");
        assert!(parse(&traversal).is_err());

        let relative = TRUSTED.replace("root = \"/\"", "root = \"tmp\"");
        assert!(parse(&relative).is_err());

        let nul = TRUSTED.replace("root = \"/\"", "root = \"/tmp\\u0000\"");
        assert!(parse(&nul).is_err());
    }

    #[test]
    fn duplicate_applications_and_duplicate_targets_are_rejected() {
        let duplicate_application = format!("{TRUSTED}\n{TRUSTED}");
        assert!(parse(&duplicate_application).is_err());

        let duplicate_target = TRUSTED.replace(
            "root = \"/\"\n        target = 3",
            "root = \"/\"\n        target = 3\n\n        [[application.view]]\n        root = \"/tmp\"\n        target = 3",
        );
        assert!(parse(&duplicate_target).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        let top_level = format!("unknown = true\n{TRUSTED}");
        assert!(parse(&top_level).is_err());

        let application = TRUSTED.replace(
            "argv = [\"/bin/android\", \"--view-handle\", \"3\"]",
            "argv = [\"/bin/android\", \"--view-handle\", \"3\"]\n        extra = true",
        );
        assert!(parse(&application).is_err());

        let view = TRUSTED.replace("target = 3", "target = 3\n        extra = true");
        assert!(parse(&view).is_err());
    }

    #[test]
    fn application_and_view_validation_is_strict() {
        let relative_executable = TRUSTED.replace("\"/bin/android\"", "\"bin/android\"");
        assert!(parse(&relative_executable).is_err());

        let empty_argv = TRUSTED.replace("[\"/bin/android\", \"--view-handle\", \"3\"]", "[]");
        assert!(parse(&empty_argv).is_err());

        let invalid_id = TRUSTED.replace("org.scarlet.Android", "org/scarlet");
        assert!(parse(&invalid_id).is_err());

        let low_target = TRUSTED.replace("target = 3", "target = 2");
        assert!(parse(&low_target).is_err());

        let high_target = TRUSTED.replace("target = 3", "target = 256");
        assert!(parse(&high_target).is_err());
    }

    #[test]
    fn size_and_count_limits_are_enforced() {
        assert!(parse(&" ".repeat(MAX_CONFIG_BYTES + 1)).is_err());

        let mut too_many_applications = String::new();
        for index in 0..=MAX_APPLICATIONS {
            too_many_applications.push_str(&format!(
                "[[application]]\nid = \"org.scarlet.App{index}\"\nargv = [\"/bin/app\"]\n[[application.view]]\nroot = \"/\"\ntarget = 3\n"
            ));
        }
        assert!(parse(&too_many_applications).is_err());

        let mut too_many_views =
            String::from("[[application]]\nid = \"org.scarlet.App\"\nargv = [\"/bin/app\"]\n");
        for target in 3..=3 + MAX_VIEWS_PER_APPLICATION {
            too_many_views.push_str(&format!(
                "[[application.view]]\nroot = \"/\"\ntarget = {target}\n"
            ));
        }
        assert!(parse(&too_many_views).is_err());

        let mut argv = vec!["/bin/app".to_string()];
        argv.extend((0..MAX_ARGUMENTS).map(|index| format!("--arg={index}")));
        let too_many_arguments = format!(
            "[[application]]\nid = \"org.scarlet.App\"\nargv = [{}]\n[[application.view]]\nroot = \"/\"\ntarget = 3\n",
            argv.iter()
                .map(|argument| format!("\"{argument}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(parse(&too_many_arguments).is_err());
    }
}
