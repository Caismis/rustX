//! A real peer echoes received credentials into every management catalog.
use rmcp::model::{
    DiscoverResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};

/// The stdio credential to echo, read only inside the spawned peer process.
pub const ECHO_ENV: &str = "RUSTX_FIXTURE_CREDENTIAL_ECHO";

/// `Some` echoes a received process environment value; `None` echoes the
/// Authorization header from the real HTTP request context.
#[derive(Clone)]
pub struct CredentialEcho(pub Option<String>);

impl CredentialEcho {
    fn received(&self, context: &RequestContext<RoleServer>) -> Result<String, rmcp::ErrorData> {
        self.0
            .clone()
            .or_else(|| {
                context
                    .extensions
                    .get::<http::request::Parts>()?
                    .headers
                    .get(http::header::AUTHORIZATION)?
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            })
            .ok_or_else(|| rmcp::ErrorData::invalid_params("credential not received by peer", None))
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
        std::future::ready(self.received(&context).map(|credential| {
        let mut tool = super::fixture_tool_named("credential-echo");
        tool.description = Some(format!("echo {credential}").into());
        tool.input_schema = std::sync::Arc::new(serde_json::json!({
            "type":"object", "properties": { (credential.clone()): {
                "type":"object", "properties":{"nested":{"description":credential.clone(),"enum":[credential]}}
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
        std::future::ready(self.received(&context).map(|credential| {
            rmcp::model::ListResourcesResult {
                resources: vec![
                    rmcp::model::Resource::new("fixture://credential", "credential")
                        .with_description(credential),
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
        std::future::ready(self.received(&context).map(|credential| {
            rmcp::model::ListResourceTemplatesResult {
                resource_templates: vec![
                    rmcp::model::ResourceTemplate::new("fixture://{name}", "credential")
                        .with_description(credential),
                ],
                ..Default::default()
            }
        }))
    }
}
