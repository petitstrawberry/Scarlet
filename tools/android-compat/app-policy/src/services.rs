//! Service access exported from the same compiled policy as the Android image.
use alloc::{collections::BTreeMap, string::String, vec::Vec};

pub struct Grant {
    pub target: String,
    pub permissions: u64,
}

pub struct Services(BTreeMap<String, Vec<Grant>>);

fn symbol(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl Services {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > 1024 * 1024 || !text.ends_with('\n') {
            return Err("service policy size or termination");
        }
        let mut lines = text.lines();
        if lines.next() != Some("scarlet-android-service-policy-v1") {
            return Err("service policy version");
        }
        let mut domains: BTreeMap<String, Vec<Grant>> = BTreeMap::new();
        let mut count = 0;
        for line in lines {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            match fields.as_slice() {
                ["domain", name] if symbol(name) => {
                    if domains.insert(String::from(*name), Vec::new()).is_some()
                        || domains.len() > 256
                    {
                        return Err("service policy domain");
                    }
                }
                ["allow", source, target, permissions] => {
                    let grants = domains.get_mut(*source).ok_or("service policy source")?;
                    let label: Vec<_> = target.split(':').collect();
                    if !matches!(label.as_slice(), ["u", "r" | "object_r", name, "s0"] if symbol(name))
                    {
                        return Err("service policy target");
                    }
                    let permissions = match *permissions {
                        "1" => 1,
                        "2" => 2,
                        "3" => 3,
                        "4" => 4,
                        "5" => 5,
                        "6" => 6,
                        "7" => 7,
                        _ => return Err("service policy permissions"),
                    };
                    count += 1;
                    if count > 16384 || grants.iter().any(|g| g.target == *target) {
                        return Err("service policy duplicate or count");
                    }
                    grants.push(Grant {
                        target: String::from(*target),
                        permissions,
                    });
                }
                _ => return Err("service policy record"),
            }
        }
        if domains.is_empty() {
            return Err("service policy domains missing");
        }
        Ok(Self(domains))
    }

    pub fn grants(&self, context: &str) -> Result<&[Grant], &'static str> {
        let fields: Vec<_> = context.split(':').collect();
        let domain = match fields.as_slice() {
            ["u", "r", domain, "s0"] => domain,
            ["u", "r", domain, "s0", categories]
                if categories.split(',').all(|c| {
                    c.strip_prefix('c')
                        .and_then(|n| n.parse::<u16>().ok())
                        .is_some_and(|n| n < 1024)
                }) =>
            {
                domain
            }
            _ => return Err("service policy subject context"),
        };
        self.0
            .get(*domain)
            .map(Vec::as_slice)
            .ok_or("service policy unmapped domain")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: &str = "scarlet-android-service-policy-v1\n\
        domain mediaprovider_app\n\
        domain untrusted_app\n\
        domain isolated_app\n\
        allow mediaprovider_app u:object_r:mediaserver_service:s0 2\n\
        allow mediaprovider_app u:object_r:hidl_manager_hwservice:s0 2\n\
        allow untrusted_app u:r:servicemanager:s0 4\n";

    #[test]
    fn exact_domain_rules_apply_to_category_labels() {
        let services = Services::parse(POLICY).unwrap();
        let media = services
            .grants("u:r:mediaprovider_app:s0:c17,c256,c512,c768")
            .unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].target, "u:object_r:mediaserver_service:s0");
        assert_eq!(media[0].permissions, 2);
        let app = services
            .grants("u:r:untrusted_app:s0:c18,c256,c512,c768")
            .unwrap();
        assert_eq!(app.len(), 1);
        assert_eq!(app[0].permissions, 4);
        assert!(services.grants("u:r:isolated_app:s0").unwrap().is_empty());
        assert!(services.grants("u:r:priv_app:s0").is_err());
        assert!(services.grants("u:r:mediaprovider_app:s1").is_err());
        assert!(services.grants("u:r:mediaprovider_app:s0:c1024").is_err());
    }

    #[test]
    fn malformed_and_duplicate_rules_are_rejected() {
        for suffix in [
            "allow mediaprovider_app u:object_r:mediaserver_service:s0 2\n",
            "allow missing u:object_r:mediaserver_service:s0 2\n",
            "allow isolated_app u:object_r:mediaserver_service:s0 8\n",
            "allow isolated_app u:object_r:mediaserver_service:s0:c1 2\n",
            "domain isolated_app\n",
        ] {
            assert!(Services::parse(&alloc::format!("{POLICY}{suffix}")).is_err());
        }
        assert!(Services::parse(POLICY.trim_end()).is_err());
    }
}
