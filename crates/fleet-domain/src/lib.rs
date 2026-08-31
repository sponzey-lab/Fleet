pub mod agent;
pub mod artifact;
pub mod audit;
pub mod capability;
pub mod catalog;
pub mod certificate;
pub mod job;
pub mod policy;
pub mod runbook;
pub mod secret;
pub mod selector;
pub mod signing;

pub use agent::*;
pub use artifact::*;
pub use audit::*;
pub use capability::*;
pub use catalog::*;
pub use certificate::*;
pub use job::*;
pub use policy::*;
pub use runbook::*;
pub use secret::*;
pub use selector::*;
pub use signing::*;

pub const DOMAIN_LAYER: &str = "fleet-domain";

#[cfg(test)]
mod catalog_fixture {
    use crate::{parse_policy_document, parse_runbook_document};

    pub struct CatalogFixture {
        policies: Vec<String>,
        runbooks: Vec<String>,
    }

    pub struct CatalogParseStats {
        pub document_count: usize,
        pub parsed_bytes: usize,
    }

    impl CatalogFixture {
        pub fn document_count(&self) -> usize {
            self.policies.len() + self.runbooks.len()
        }

        pub fn total_bytes(&self) -> usize {
            self.policies
                .iter()
                .chain(&self.runbooks)
                .map(String::len)
                .sum()
        }

        pub fn parse_all(&self) -> CatalogParseStats {
            let mut document_count = 0;
            let mut parsed_bytes = 0;

            for document in &self.policies {
                parse_policy_document(document).expect("catalog fixture policy must be valid");
                document_count += 1;
                parsed_bytes += document.len();
            }
            for document in &self.runbooks {
                parse_runbook_document(document).expect("catalog fixture runbook must be valid");
                document_count += 1;
                parsed_bytes += document.len();
            }

            CatalogParseStats {
                document_count,
                parsed_bytes,
            }
        }
    }

    pub fn one_thousand_documents() -> CatalogFixture {
        let policies = (0..500)
            .map(|index| {
                format!(
                    "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Policy\nmetadata:\n  id: catalog-policy-{index}\n  name: catalog-policy-{index}\n  version: 1\nspec:\n  selector:\n    matchLabels:\n      role: web\n  checks:\n    - id: nginx-service\n      service:\n        name: nginx\n        state: running\n"
                )
            })
            .collect();
        let runbooks = (0..500)
            .map(|index| {
                format!(
                    "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nname: catalog-runbook-{index}\nselector: role=web\nsteps:\n  - id: nginx-package\n    package:\n      name: nginx\n      state: present\n"
                )
            })
            .collect();

        CatalogFixture { policies, runbooks }
    }
}

#[cfg(test)]
mod catalog_fixture_tests {
    use crate::catalog_fixture;

    #[test]
    fn catalog_fixture_contains_one_thousand_valid_documents_with_counted_bytes() {
        let fixture = catalog_fixture::one_thousand_documents();
        let stats = fixture.parse_all();

        assert_eq!(fixture.document_count(), 1_000);
        assert!(fixture.total_bytes() > 0);
        assert_eq!(stats.document_count, fixture.document_count());
        assert_eq!(stats.parsed_bytes, fixture.total_bytes());
    }
}
