//! A real peer echoes received credentials into every management catalog.
use rmcp::model::{
    DiscoverResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};

/// The stdio credential to echo, read only inside the spawned peer process.
pub const ECHO_ENV: &str = "RUSTX_FIXTURE_CREDENTIAL_ECHO";

/// `Some` echoes a received process environment value; `None` echoes the
/// Authorization or Proxy-Authorization header from the real HTTP request context.
#[derive(Clone)]
pub struct CredentialEcho(pub Option<String>);

impl CredentialEcho {
    fn received(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<(String, String), rmcp::ErrorData> {
        let credential = self
            .0
            .clone()
            .or_else(|| {
                let headers = &context.extensions.get::<http::request::Parts>()?.headers;
                headers
                    .get(http::header::AUTHORIZATION)
                    .or_else(|| headers.get(http::header::PROXY_AUTHORIZATION))?
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            })
            .ok_or_else(|| {
                rmcp::ErrorData::invalid_params("credential not received by peer", None)
            })?;
        let mut parts = credential.split_ascii_whitespace();
        let token = if parts
            .next()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Bearer"))
        {
            parts.next().unwrap_or(&credential)
        } else {
            &credential
        }
        .to_owned();
        Ok((credential, token))
    }
}

impl ServerHandler for CredentialEcho {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, rmcp::ErrorData>> + Send {
        std::future::ready(Ok(DiscoverResult::from_server_info(
            vec![ProtocolVersion::LATEST],
            self.get_info(),
        )))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + Send
    {
        std::future::ready(self.received(&context).map(|(credential, token)| {
        let mut tool = super::fixture_tool_named("credential-echo");
        tool.description = Some(format!("echo {credential}; bearer {token}").into());
        tool.input_schema = std::sync::Arc::new(serde_json::json!({
            "type":"object", "properties": { (credential.clone()): {
                "type":"object", "properties":{"nested":{"description":credential.clone(),"enum":[credential.clone(),token.clone()]}}
            }, (token.clone()): {
                "type":"object", "properties":{"nested":{"description":token.clone(),"enum":[credential,token]}}
            }}
        }).as_object().unwrap().clone());
        rmcp::model::ListToolsResult {
            tools: vec![tool],
            ..Default::default()
        }
        }))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListResourcesResult, rmcp::ErrorData>>
    + Send {
        std::future::ready(self.received(&context).map(|(credential, token)| {
            rmcp::model::ListResourcesResult {
                resources: vec![
                    rmcp::model::Resource::new("fixture://credential", "credential")
                        .with_description(format!("{credential}; bearer {token}")),
                ],
                ..Default::default()
            }
        }))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<
        Output = Result<rmcp::model::ListResourceTemplatesResult, rmcp::ErrorData>,
    > + Send {
        std::future::ready(self.received(&context).map(|(credential, token)| {
            rmcp::model::ListResourceTemplatesResult {
                resource_templates: vec![
                    rmcp::model::ResourceTemplate::new("fixture://{name}", "credential")
                        .with_description(format!("{credential}; bearer {token}")),
                ],
                ..Default::default()
            }
        }))
    }
}
