//! Explicit Client grants for values used by Workspace HTTP MCP sources.

use std::collections::{BTreeMap, BTreeSet};

use rustx_execution_protocol::mcp_environment::{MAX_VALUE_BYTES, Reply, Request, valid_name};

#[derive(Default)]
pub(crate) struct Grants(BTreeSet<(String, String, String)>);

impl Grants {
    pub(crate) fn insert(&mut self, service: String, origin: String, name: String) -> bool {
        if service.is_empty()
            || service.len() > 256
            || origin.is_empty()
            || origin.len() > 2048
            || !valid_name(&name)
            || self.0.len() >= 1024
        {
            return false;
        }
        self.0.insert((service, origin, name));
        true
    }

    pub(crate) fn resolve(&self, request: &Request) -> Option<Reply> {
        self.resolve_with(request, |name| std::env::var(name).ok())
    }

    fn resolve_with(
        &self,
        request: &Request,
        get: impl Fn(&str) -> Option<String>,
    ) -> Option<Reply> {
        if !request.valid()
            || request.names.iter().any(|name| {
                !self.0.contains(&(
                    request.service.clone(),
                    request.origin.clone(),
                    name.clone(),
                ))
            })
        {
            return None;
        }
        let mut values = BTreeMap::new();
        let mut total = 0usize;
        for name in &request.names {
            let value = get(name)?;
            total = total.checked_add(value.len())?;
            if value.is_empty() || value.contains('\0') || total > MAX_VALUE_BYTES {
                return None;
            }
            values.insert(name.clone(), value);
        }
        Some(Reply { values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_scoped_to_source_origin_and_requested_names() {
        let mut grants = Grants::default();
        assert!(grants.insert(
            "search".into(),
            "https://example.test".into(),
            "TOKEN".into()
        ));
        let mut request = Request {
            service: "search".into(),
            origin: "https://example.test".into(),
            names: BTreeSet::from(["TOKEN".into()]),
        };
        let reply = grants
            .resolve_with(&request, |_| Some("secret".into()))
            .unwrap();
        assert_eq!(reply.values.len(), 1);
        assert!(!format!("{reply:?}").contains("secret"));
        request.origin = "https://other.test".into();
        assert!(
            grants
                .resolve_with(&request, |_| Some("secret".into()))
                .is_none()
        );
        request.origin = "https://example.test".into();
        request.names.insert("UNGRANTED".into());
        assert!(
            grants
                .resolve_with(&request, |_| Some("secret".into()))
                .is_none()
        );
    }
}
