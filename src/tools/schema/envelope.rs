//! Close business schemas over their original resources before putting them
//! beneath an invocation envelope. Metadata never becomes part of a business
//! object, including when a reference returns to that object's root.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::LazyLock,
};

use referencing::{Draft, Registry, Resolver, Resource};
use serde_json::{Map, Value, json};

use super::{SchemaError, build_validator};

/// A canonical validator and its closed model-facing resource graph.
pub(super) struct CompiledBusiness {
    pub root_uri: String,
    pub resources: Value,
    pub validator: jsonschema::Validator,
}

/// Validate the owner-supplied business document and freeze its bundled resources.
///
/// The caller places `resources` in a Draft 7 `definitions` object and
/// references `root_uri` from `arguments`. Business validation uses the returned
/// unchanged canonical validator. Both use only resource bytes supplied in the
/// document. Missing resources fail without file or network access, including
/// during registration; URI contents cannot grant host resource authority.
pub(super) fn compile_business_resources(
    canonical: &Value,
) -> Result<CompiledBusiness, SchemaError> {
    // Validate the unchanged document, including structural errors in siblings
    // ignored by legacy $ref evaluation. Both compilations are strictly no-I/O.
    let validator = build_validator(canonical).map_err(invalid_schema)?;
    let resource = Resource::from_contents(canonical.clone()).map_err(invalid_schema)?;
    let root_uri = resource.id().unwrap_or("json-schema:///").to_owned();
    let registry = Registry::options()
        .retriever(Box::new(referencing::DefaultRetriever))
        .try_new(root_uri.clone(), resource)
        .map_err(invalid_schema)?;
    let resolver = registry.try_resolver(&root_uri).map_err(invalid_schema)?;
    let root_uri = resolver.base_uri().as_str().to_owned();
    let mut graph = ResourceGraph {
        registry: &registry,
        keys: HashMap::new(),
        resources: BTreeMap::new(),
        dynamic_names: BTreeSet::new(),
    };
    graph.ensure_resource(&resolver)?;
    graph.close_dynamic_anchors()?;
    Ok(CompiledBusiness {
        root_uri,
        resources: graph.finish()?,
        validator,
    })
}

/// Validate a declaration's schema structure without resolving its references.
/// Catalog observation uses this check; admitting an executable registration
/// additionally compiles the complete business contract above.
pub(super) fn validate_declaration(schema: &Value) -> Result<(), SchemaError> {
    let draft = Draft::default().detect(schema).map_err(invalid_schema)?;
    let validator = METASCHEMA_VALIDATORS
        .get(&draft)
        .ok_or_else(|| SchemaError::InvalidSchema(format!("unsupported schema dialect {draft:?}")))?
        .as_ref()
        .map_err(|message| SchemaError::InvalidSchema(message.clone()))?;
    validator.validate(schema).map_err(invalid_schema)
}

static METASCHEMA_VALIDATORS: LazyLock<BTreeMap<Draft, Result<jsonschema::Validator, String>>> =
    LazyLock::new(|| {
        [
            (Draft::Draft4, &*referencing::meta::DRAFT4),
            (Draft::Draft6, &*referencing::meta::DRAFT6),
            (Draft::Draft7, &*referencing::meta::DRAFT7),
            (Draft::Draft201909, &*referencing::meta::DRAFT201909),
            (Draft::Draft202012, &*referencing::meta::DRAFT202012),
        ]
        .into_iter()
        .map(|(draft, schema)| {
            let validator = build_validator(schema).map_err(|error| error.to_string());
            (draft, validator)
        })
        .collect()
    });

struct ResourceNodes<'a> {
    original_root: &'a Value,
    draft: Draft,
    root_name: Option<String>,
    nodes: Map<String, Value>,
}

/// Reference resolution always uses the original registry. Emitted nodes are
/// grouped by that registry's resource identity, preserving dialect and dynamic
/// scope while replacing static references with their resolved node addresses.
struct ResourceGraph<'a> {
    registry: &'a Registry,
    keys: HashMap<(usize, String, Draft), String>,
    resources: BTreeMap<String, ResourceNodes<'a>>,
    dynamic_names: BTreeSet<String>,
}

impl<'a> ResourceGraph<'a> {
    fn ensure_resource(&mut self, resolver: &Resolver<'a>) -> Result<(), SchemaError> {
        let uri = resolver.base_uri().as_str().to_owned();
        if self.resources.contains_key(&uri) {
            return Ok(());
        }
        let root = resolver.lookup("#").map_err(invalid_schema)?;
        self.resources.insert(
            uri.clone(),
            ResourceNodes {
                original_root: root.contents(),
                draft: root.draft(),
                root_name: None,
                nodes: Map::new(),
            },
        );
        // Register the resource before compiling its root so recursive root
        // references find the same resource rather than creating a new one.
        let name = self.target(root.contents(), root.resolver(), root.draft())?;
        self.resource_mut(&uri)?.root_name = Some(name);
        Ok(())
    }

    fn target(
        &mut self,
        source: &'a Value,
        resolver: &Resolver<'a>,
        draft: Draft,
    ) -> Result<String, SchemaError> {
        self.ensure_resource(resolver)?;
        let uri = resolver.base_uri().as_str().to_owned();
        let key = (std::ptr::from_ref(source) as usize, uri.clone(), draft);
        if let Some(name) = self.keys.get(&key) {
            return Ok(name.clone());
        }
        let name = format!("schema{}", self.keys.len());
        self.keys.insert(key, name.clone());
        // Install the address before following references, including cycles.
        self.resource_mut(&uri)?
            .nodes
            .insert(name.clone(), json!({}));
        let node = self.node(source, resolver, draft, true)?;
        self.resource_mut(&uri)?.nodes.insert(name.clone(), node);
        Ok(name)
    }

    fn reference(
        &mut self,
        source: &'a Value,
        resolver: &Resolver<'a>,
        draft: Draft,
    ) -> Result<Value, SchemaError> {
        let name = self.target(source, resolver, draft)?;
        Ok(json!({"$ref": format!("{}#/definitions/{name}", resolver.base_uri().as_str())}))
    }

    fn node(
        &mut self,
        source: &'a Value,
        parent: &Resolver<'a>,
        inherited: Draft,
        referenced: bool,
    ) -> Result<Value, SchemaError> {
        // A lookup has already established its target dialect and URI context.
        // Only inline children need to enter a new subresource.
        let (draft, resolver) = if referenced {
            (inherited, parent.clone())
        } else {
            let draft = inherited.detect(source).map_err(invalid_schema)?;
            let resolver = parent
                .in_subresource(draft.create_resource_ref(source))
                .map_err(invalid_schema)?;
            (draft, resolver)
        };
        // Keep inline booleans syntactically intact. Some applicators select
        // their behavior from a sibling's literal form (notably `items`).
        if source.is_boolean() {
            return Ok(source.clone());
        }
        if !referenced {
            return self.reference(source, &resolver, draft);
        }
        let legacy = matches!(draft, Draft::Draft4 | Draft::Draft6 | Draft::Draft7);
        if legacy && let Some(reference) = source.get("$ref").and_then(Value::as_str) {
            // These dialects ignore every sibling of an active $ref, including
            // an id. Moving a document and injecting an id would change that.
            return self.static_reference(&resolver, reference);
        }
        let mut emitted = source.clone();
        if let Some(object) = emitted.as_object_mut() {
            remove_inert_schemas(source, object);
            object.remove("id");
            object.remove("$id");
            let resource = self
                .resources
                .get(resolver.base_uri().as_str())
                .ok_or_else(|| {
                    SchemaError::InvalidSchema(
                        "business resource disappeared during compilation".into(),
                    )
                })?;
            if std::ptr::eq(resource.original_root, source) {
                object.remove("$dynamicAnchor");
                object.remove("$recursiveAnchor");
            }
        }
        if let Some(reference) = source.get("$ref").and_then(Value::as_str) {
            let rewritten = self.static_reference(&resolver, reference)?;
            emitted["$ref"] = rewritten["$ref"].clone();
        }
        if draft == Draft::Draft202012
            && let Some(reference) = source.get("$dynamicRef").and_then(Value::as_str)
        {
            self.dynamic_reference(&resolver, reference, &mut emitted)?;
        }
        let mut children: Vec<_> = draft.subresources_of(source).collect();
        // The validator also executes these legacy applicators in modern
        // dialects, although referencing's discovery iterator omits them.
        if let Some(dependencies) = source.get("dependencies").and_then(Value::as_object) {
            children.extend(dependencies.values().filter(|schema| schema.is_object()));
        }
        if source.get("items").is_some_and(Value::is_array)
            && let Some(additional) = source.get("additionalItems")
        {
            children.push(additional);
        }
        self.children(source, &mut emitted, &children, &resolver, draft)?;
        Ok(emitted)
    }

    fn static_reference(
        &mut self,
        resolver: &Resolver<'a>,
        reference: &str,
    ) -> Result<Value, SchemaError> {
        match resolver.lookup(reference) {
            Ok(target) => self.reference(target.contents(), target.resolver(), target.draft()),
            // The unchanged canonical validator has already accepted this
            // document. An unresolved ref can therefore occur only where the
            // engine does not execute it, such as `if` without then/else and
            // without an observing unevaluated collector. Retain the original
            // ref in the same resource and let the engine keep that decision.
            Err(_) => Ok(json!({"$ref": reference})),
        }
    }

    fn dynamic_reference(
        &mut self,
        resolver: &Resolver<'a>,
        reference: &str,
        emitted: &mut Value,
    ) -> Result<(), SchemaError> {
        // Resolve the lexical target without one call path's dynamic scope;
        // the emitted engine must choose each invocation's override itself.
        let lexical = self
            .registry
            .try_resolver(resolver.base_uri().as_str())
            .map_err(invalid_schema)?;
        let Ok(target) = lexical.lookup(reference) else {
            // As with a static ref, canonical compilation has already proved
            // that an unresolved dynamic ref is not executed.
            return Ok(());
        };
        self.target(target.contents(), target.resolver(), target.draft())?;
        let absolute = referencing::uri::resolve_against(&resolver.base_uri().borrow(), reference)
            .map_err(invalid_schema)?;
        let fragment = absolute.fragment().map_or("", |fragment| fragment.as_str());
        if fragment.is_empty() || fragment.starts_with('/') {
            let rewritten = self.reference(target.contents(), target.resolver(), target.draft())?;
            emitted["$dynamicRef"] = rewritten["$ref"].clone();
        } else {
            self.dynamic_names.insert(fragment.to_owned());
        }
        // Named dynamic anchors must retain their name and resource scope.
        Ok(())
    }

    fn close_dynamic_anchors(&mut self) -> Result<(), SchemaError> {
        loop {
            let previous = self.keys.len();
            let resources: Vec<_> = self.resources.keys().cloned().collect();
            let names: Vec<_> = self.dynamic_names.iter().cloned().collect();
            for uri in resources {
                let resolver = self.registry.try_resolver(&uri).map_err(invalid_schema)?;
                for name in &names {
                    if let Ok(target) = resolver.lookup(&format!("#{name}"))
                        && target
                            .contents()
                            .get("$dynamicAnchor")
                            .and_then(Value::as_str)
                            == Some(name.as_str())
                    {
                        self.target(target.contents(), target.resolver(), target.draft())?;
                    }
                }
            }
            if self.keys.len() == previous {
                return Ok(());
            }
        }
    }

    fn children(
        &mut self,
        original: &'a Value,
        emitted: &mut Value,
        candidates: &[&'a Value],
        resolver: &Resolver<'a>,
        draft: Draft,
    ) -> Result<(), SchemaError> {
        match (original, emitted) {
            (Value::Object(source), Value::Object(output)) => {
                for (key, value) in source {
                    if let Some(target) = output.get_mut(key) {
                        self.child(value, target, candidates, resolver, draft)?;
                    }
                }
            }
            (Value::Array(source), Value::Array(output)) => {
                for (value, target) in source.iter().zip(output) {
                    self.child(value, target, candidates, resolver, draft)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn child(
        &mut self,
        original: &'a Value,
        emitted: &mut Value,
        candidates: &[&'a Value],
        resolver: &Resolver<'a>,
        draft: Draft,
    ) -> Result<(), SchemaError> {
        if emitted.is_boolean() {
            return Ok(());
        }
        if candidates.iter().any(|node| std::ptr::eq(*node, original)) {
            *emitted = self.node(original, resolver, draft, false)?;
            Ok(())
        } else {
            // Traverse containers to locate actual schema children, never
            // interpreting const/enum/default business data as a schema.
            self.children(original, emitted, candidates, resolver, draft)
        }
    }

    fn resource_mut(&mut self, uri: &str) -> Result<&mut ResourceNodes<'a>, SchemaError> {
        self.resources.get_mut(uri).ok_or_else(|| {
            SchemaError::InvalidSchema(format!("missing compiled business resource {uri:?}"))
        })
    }

    fn finish(self) -> Result<Value, SchemaError> {
        let mut resources = Map::new();
        for (uri, nodes) in self.resources {
            let root_name = nodes.root_name.ok_or_else(|| {
                SchemaError::InvalidSchema(format!("missing business root for {uri:?}"))
            })?;
            // An outer metaschema validates every schema under `definitions`
            // using its own dialect. Keep authored assertions in an annotation
            // on their original resource; its schema index contains only refs
            // and anchors, valid in every supported dialect. Following an index
            // reference still enters the original resource and dialect.
            let mut index = Map::new();
            for (name, node) in &nodes.nodes {
                let mut entry = json!({"allOf": [{
                    "$ref": format!("{uri}#/x-rustx-schema-nodes/{name}")
                }]});
                for key in ["$schema", "$anchor", "$dynamicAnchor", "$recursiveAnchor"] {
                    if let Some(value) = node.get(key) {
                        entry[key] = value.clone();
                    }
                }
                index.insert(name.clone(), entry);
            }
            let mut resource = json!({
                "$schema": dialect_uri(nodes.draft)?,
                "definitions": index,
                "x-rustx-schema-nodes": nodes.nodes,
                "allOf": [{"$ref": format!("{uri}#/definitions/{root_name}")}],
            });
            resource[if nodes.draft == Draft::Draft4 {
                "id"
            } else {
                "$id"
            }] = json!(uri);
            for key in ["$dynamicAnchor", "$recursiveAnchor"] {
                if let Some(value) = nodes.original_root.get(key) {
                    resource[key] = value.clone();
                }
            }
            resources.insert(uri, resource);
        }
        Ok(Value::Object(resources))
    }
}

/// Declarations and inactive applicators are not eagerly executable schemas.
/// Their original locations remain resolvable in the captured registry, so a
/// real reference can still compile one without compiling its unused peers.
fn remove_inert_schemas(original: &Value, emitted: &mut Map<String, Value>) {
    for name in ["definitions", "$defs", "contentSchema"] {
        emitted.remove(name);
    }
    if original.get("if").is_none() {
        emitted.remove("then");
        emitted.remove("else");
    }
    match original.get("items") {
        Some(Value::Array(_)) => {}
        // The current validator selects its false-items behavior without
        // compiling additionalItems. Preserve that literal relationship.
        Some(Value::Bool(false)) => {
            if let Some(additional) = emitted.get_mut("additionalItems") {
                *additional = Value::Bool(false);
            }
        }
        _ => {
            emitted.remove("additionalItems");
        }
    }
}

fn dialect_uri(draft: Draft) -> Result<&'static str, SchemaError> {
    match draft {
        Draft::Draft4 => Ok("http://json-schema.org/draft-04/schema#"),
        Draft::Draft6 => Ok("http://json-schema.org/draft-06/schema#"),
        Draft::Draft7 => Ok("http://json-schema.org/draft-07/schema#"),
        Draft::Draft201909 => Ok("https://json-schema.org/draft/2019-09/schema"),
        Draft::Draft202012 => Ok("https://json-schema.org/draft/2020-12/schema"),
        _ => Err(SchemaError::InvalidSchema(format!(
            "unsupported schema dialect {draft:?}"
        ))),
    }
}

fn invalid_schema(error: impl std::fmt::Display) -> SchemaError {
    SchemaError::InvalidSchema(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(canonical: &Value) -> Value {
        envelope_for(
            &compile_business_resources(canonical).unwrap_or_else(|error| {
                panic!("business resources do not close: {canonical}: {error:?}")
            }),
        )
    }

    fn envelope_for(business: &CompiledBusiness) -> Value {
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$id": "urn:rustx:envelope-test",
            "definitions": business.resources,
            "type": "object",
            "properties": {
                "arguments": {"$ref": business.root_uri},
                "elevation_reason": {"type": ["string", "null"]},
            },
            "required": ["arguments"],
            "additionalProperties": false,
        })
    }

    fn assert_projection(canonical: &Value, values: &[Value]) {
        let original = build_validator(canonical).expect("canonical schema validates");
        let compiled = build_validator(&envelope(canonical)).expect("envelope validates");
        for business in values {
            for reason in [
                None,
                Some(json!(null)),
                Some(json!("user requested the operation")),
            ] {
                let mut invocation = json!({"arguments": business});
                if let Some(reason) = reason {
                    invocation["elevation_reason"] = reason;
                }
                assert_eq!(
                    original.is_valid(business),
                    compiled.is_valid(&invocation),
                    "business validity changed: schema={canonical}, call={invocation}",
                );
            }
        }
    }

    #[test]
    fn business_object_constraints_do_not_constrain_runtime_metadata() {
        for canonical in [
            json!({"type": "object", "enum": [{}]}),
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
                "propertyNames": {"pattern": "^[a-z]+$"},
                "additionalProperties": false,
            }),
            json!({
                "type": "object",
                "patternProperties": {"^elevation_reason$": {"type": "integer"}},
                "maxProperties": 1,
                "additionalProperties": false,
            }),
        ] {
            assert_projection(
                &canonical,
                &[
                    json!({}),
                    json!({"command": "status"}),
                    json!({"elevation_reason": 1}),
                    json!({"elevation_reason": "business"}),
                    json!({"command": "status", "extra": true}),
                ],
            );
        }
    }

    #[test]
    fn legacy_references_keep_ignored_siblings_recursion_and_literal_data() {
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
        ] {
            let canonical = json!({
                "$schema": dialect,
                "type": "object",
                "$id": "https://ignored.example.test/",
                "$ref": "#/definitions/body",
                "required": ["ignored"],
                "definitions": {"body": {
                    "type": "object",
                    "properties": {
                        "child": {"$ref": "#"},
                        "literal": {"enum": [{"$ref": "#"}]},
                    },
                    "additionalProperties": false,
                }},
            });
            assert_projection(
                &canonical,
                &[
                    json!({}),
                    json!({"child": {"child": {}}}),
                    json!({"literal": {"$ref": "#"}}),
                    json!({"literal": {"$ref": "changed"}}),
                    json!({"child": {"elevation_reason": "business"}}),
                ],
            );
        }
    }

    #[test]
    fn modern_resources_keep_conjunction_and_dynamic_anchor_scope() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.test/root",
            "type": "object",
            "$ref": "#/$defs/body",
            "required": ["left", "right"],
            "$defs": {
                "body": {
                    "type": "object",
                    "properties": {
                        "left": {"$ref": "https://example.test/left"},
                        "right": {"$ref": "https://example.test/right"},
                    },
                    "additionalProperties": false,
                },
                "left": {
                    "$id": "https://example.test/left", "$dynamicAnchor": "node",
                    "type": "object",
                    "properties": {"a": {"type": "integer"}, "child": {"$dynamicRef": "#node"}},
                    "additionalProperties": false,
                },
                "right": {
                    "$id": "https://example.test/right", "$dynamicAnchor": "node",
                    "type": "object",
                    "properties": {"b": {"type": "string"}, "child": {"$dynamicRef": "#node"}},
                    "additionalProperties": false,
                },
            },
        });
        assert_projection(
            &canonical,
            &[
                json!({}),
                json!({"left": {}, "right": {}}),
                json!({"left": {"child": {"a": 1}}, "right": {"child": {"b": "yes"}}}),
                json!({"left": {"child": {"b": "bad"}}, "right": {"child": {"a": 1}}}),
            ],
        );
    }

    #[test]
    fn dynamic_and_recursive_anchor_overrides_survive_a_strict_resource() {
        for (dialect, anchor, value, reference, target) in [
            (
                "https://json-schema.org/draft/2020-12/schema",
                "$dynamicAnchor",
                json!("node"),
                "$dynamicRef",
                "#node",
            ),
            (
                "https://json-schema.org/draft/2019-09/schema",
                "$recursiveAnchor",
                json!(true),
                "$recursiveRef",
                "#",
            ),
        ] {
            let mut canonical = json!({
                "$schema": dialect, "$id": "https://example.test/strict-tree",
                "type": "object", "$ref": "https://example.test/tree",
                "unevaluatedProperties": false,
                "$defs": {"tree": {
                    "$id": "https://example.test/tree", "type": "object",
                    "properties": {
                        "data": {"type": "integer"},
                        "children": {"type": "array", "items": {}},
                    },
                }},
            });
            canonical[anchor] = value.clone();
            canonical["$defs"]["tree"][anchor] = value;
            canonical["$defs"]["tree"]["properties"]["children"]["items"][reference] =
                json!(target);
            assert_projection(
                &canonical,
                &[
                    json!({}),
                    json!({"data": 1}),
                    json!({"data": 1, "extra": true}),
                    json!({"children": [{"data": 1}]}),
                    json!({"children": [{"data": 1, "extra": true}]}),
                ],
            );
        }
    }

    #[test]
    fn repeated_resource_calls_keep_distinct_dynamic_override_scopes() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.test/root", "type": "object",
            "properties": {
                "left": {"$ref": "https://example.test/left"},
                "right": {"$ref": "https://example.test/right"},
            },
            "additionalProperties": false,
            "$defs": {
                "tree": {
                    "$id": "https://example.test/tree", "$dynamicAnchor": "node",
                    "type": "object", "properties": {
                        "data": {}, "child": {"$dynamicRef": "#node"},
                    },
                    "additionalProperties": false,
                },
                "left": {
                    "$id": "https://example.test/left", "$dynamicAnchor": "node",
                    "$ref": "https://example.test/tree",
                    "properties": {"data": {"type": "integer"}},
                },
                "right": {
                    "$id": "https://example.test/right", "$dynamicAnchor": "node",
                    "$ref": "https://example.test/tree",
                    "properties": {"data": {"type": "string"}},
                },
            },
        });
        assert_projection(
            &canonical,
            &[
                json!({"left": {"child": {"data": 1}}, "right": {"child": {"data": "yes"}}}),
                json!({"left": {"child": {"data": "bad"}}, "right": {"child": {"data": 1}}}),
                json!({"left": {"child": {"child": {"data": 1}}}}),
                json!({"right": {"child": {"child": {"data": "yes"}}}}),
            ],
        );
    }

    #[test]
    fn relative_resource_ids_keep_the_original_resolution_base() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "root.json",
            "type": "object",
            "properties": {"child": {"$ref": "child.json"}},
            "additionalProperties": false,
            "$defs": {"child": {
                "$id": "child.json",
                "type": "object",
                "properties": {"next": {"$ref": "#"}},
                "additionalProperties": false,
            }},
        });
        assert_projection(
            &canonical,
            &[
                json!({}),
                json!({"child": {"next": {}}}),
                json!({"child": {"next": {"extra": true}}}),
            ],
        );
    }

    #[test]
    fn draft201909_keeps_dynamic_ref_as_an_unevaluated_annotation() {
        for reference in ["#/missing", "http://127.0.0.1:1/never-retrieved.json"] {
            assert_projection(
                &json!({
                    "$schema": "https://json-schema.org/draft/2019-09/schema",
                    "type": "object", "$dynamicRef": reference,
                    "properties": {"amount": {"type": "integer"}},
                    "additionalProperties": false,
                }),
                &[json!({}), json!({"amount": 1}), json!({"amount": "bad"})],
            );
        }
    }

    #[test]
    fn draft4_exclusive_bounds_remain_valid_inside_the_modern_envelope() {
        let canonical = json!({
            "$schema": "http://json-schema.org/draft-04/schema#", "type": "object",
            "properties": {"amount": {
                "type": "number", "minimum": 1, "exclusiveMinimum": true,
                "maximum": 3, "exclusiveMaximum": false,
            }},
            "required": ["amount"], "additionalProperties": false,
        });
        assert_projection(
            &canonical,
            &[
                json!({}),
                json!({"amount": 1}),
                json!({"amount": 2}),
                json!({"amount": 3}),
                json!({"amount": 4}),
            ],
        );
    }

    #[test]
    fn owner_bundled_resources_keep_legacy_bounds_and_recursive_scope() {
        for mut target in [
            json!({
                "$schema": "http://json-schema.org/draft-04/schema#", "type": "object",
                "properties": {"amount": {
                    "type": "number", "minimum": 1, "exclusiveMinimum": true,
                }}, "required": ["amount"], "additionalProperties": false,
            }),
            json!({
                "$schema": "https://json-schema.org/draft/2019-09/schema",
                "$recursiveAnchor": true, "type": "object",
                "properties": {
                    "amount": {"type": "integer", "minimum": 2},
                    "child": {"$recursiveRef": "#"},
                }, "required": ["amount"], "additionalProperties": false,
            }),
        ] {
            let uri = "https://example.test/owner-captured-business.json";
            let id = if target["$schema"] == "http://json-schema.org/draft-04/schema#" {
                "id"
            } else {
                "$id"
            };
            target[id] = json!(uri);
            // The outer metaschema validates bundled schema positions. Use a
            // Draft 4 container that accepts both the legacy boolean bounds and
            // the explicitly identified modern recursive resource. Putting a
            // Draft 4 boolean exclusiveMinimum under a modern $defs would make
            // the unchanged canonical document itself invalid in this engine.
            let canonical = json!({
                "$schema": "http://json-schema.org/draft-04/schema#", "type": "object",
                "$ref": uri, "definitions": {"captured": target},
            });
            let original = build_validator(&canonical).expect("valid owner-supplied bundle");
            let business = compile_business_resources(&canonical).unwrap();
            let compiled = envelope_for(&business);
            let validator = build_validator(&compiled).unwrap();
            for value in [
                json!({}),
                json!({"amount": 1}),
                json!({"amount": 2}),
                json!({"amount": 2, "child": {"amount": 1}}),
                json!({"amount": 2, "child": {"amount": 2}}),
            ] {
                let valid = value.get("amount") == Some(&json!(2))
                    && (value.get("child").is_none()
                        || (id == "$id" && value["child"]["amount"] == 2));
                assert_eq!(
                    original.is_valid(&value),
                    valid,
                    "canonical: {target}, {value}"
                );
                assert_eq!(business.validator.is_valid(&value), valid);
                assert_eq!(
                    validator.is_valid(&json!({"arguments": value})),
                    valid,
                    "bundled dialect or recursive scope changed: {target}, {value}",
                );
            }
        }
    }

    #[tokio::test]
    async fn owner_captured_resources_are_frozen_without_host_retrieval() {
        let directory = tempfile::tempdir().expect("owner resource directory");
        let path = directory.path().join("business.json");
        let file_uri = format!("file://{}", path.display());
        let http_uri = "https://schema.example.test/business.json";
        let target = json!({"type": "object", "required": ["command"], "properties": {
            "command": {"type": "string"},
        }, "additionalProperties": false});
        std::fs::write(&path, target.to_string()).expect("write owner resource");
        // Capturing bytes is an explicit owner operation, outside compilation.
        // URI identities alone never trigger another read of this file.
        let captured: Value = serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
        let mut file_resource = captured.clone();
        file_resource["$id"] = json!(file_uri);
        let mut http_resource = captured;
        http_resource["$id"] = json!(http_uri);
        let canonical = json!({"type": "object", "allOf": [
            {"$ref": file_uri}, {"$ref": http_uri},
        ], "$defs": {"file": file_resource, "http": http_resource}});
        let business = compile_business_resources(&canonical).expect("compile captured resources");
        let compiled = envelope_for(&business);
        // Mutating and removing the original has no effect on either artifact.
        std::fs::write(&path, r#"{"type":"object","required":["replacement"]}"#).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(business.validator.is_valid(&json!({"command": "status"})));
        assert!(!business.validator.is_valid(&json!({"command": 1})));
        for _ in 0..2 {
            let validator = build_validator(&compiled).expect("closed envelope validates");
            assert!(validator.is_valid(&json!({"arguments": {"command": "status"}})));
            assert!(!validator.is_valid(&json!({"arguments": {"command": 1}})));
        }
    }

    #[test]
    fn uncaptured_file_and_network_resources_are_rejected_before_host_access() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private.json");
        let marker = "local-content-must-not-leave-76fc114a";
        std::fs::write(&path, json!({"x-private-marker": marker}).to_string()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        for uri in [
            format!("file://{}", path.display()),
            format!("http://{}/private.json", listener.local_addr().unwrap()),
            format!("https://{}/private.json", listener.local_addr().unwrap()),
        ] {
            let canonical = json!({"type": "object", "$ref": uri});
            // Catalog structure is observable without authorizing this resource.
            validate_declaration(&canonical).unwrap();
            let error = compile_business_resources(&canonical)
                .err()
                .expect("URI is not resource-read authority");
            assert!(error.to_string().contains("does not fetch resources"));
            assert!(!error.to_string().contains(marker));
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }

    #[test]
    fn nested_recursive_resources_keep_unevaluated_item_reference_identity() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2019-09/schema",
            "type": "object", "required": ["value"], "additionalProperties": false,
            "properties": {"value": {
                "$id": "https://example.test/trees/extended", "$recursiveAnchor": true,
                "$ref": "./tree", "items": [true, true, {"type": "string"}],
                "$defs": {"tree": {
                    "$id": "./tree", "$recursiveAnchor": true, "type": "array",
                    "items": [{"type": "number"}, {
                        "$recursiveRef": "#", "unevaluatedItems": false,
                    }],
                }},
            }},
        });
        assert_projection(
            &canonical,
            &[
                json!({"value": [1, []]}),
                json!({"value": [1, [2, []], "outer"]}),
                json!({"value": [1, [2, [], "inner"], "outer"]}),
                json!({"value": [1, [2, [], true], "outer"]}),
                json!({"value": ["bad", []]}),
            ],
        );
    }

    #[test]
    fn unused_declarations_and_inactive_applicators_do_not_execute_references() {
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ] {
            for keyword in [
                "definitions",
                "$defs",
                "contentSchema",
                "if",
                "then",
                "else",
                "additionalItems",
            ] {
                let mut canonical = json!({"$schema": dialect, "type": "object"});
                canonical[keyword] = if matches!(keyword, "definitions" | "$defs") {
                    json!({"unused": {"$ref": "#/missing"}})
                } else {
                    json!({"$ref": "#/missing"})
                };
                assert_projection(&canonical, &[json!({}), json!({"value": 1})]);
            }
            for items in [json!({}), json!(true), json!(false)] {
                if dialect.contains("draft-04") && items.is_boolean() {
                    continue;
                }
                let canonical = json!({
                    "$schema": dialect, "type": "object", "items": items,
                    "additionalItems": {"$ref": "#/missing"},
                });
                assert_projection(&canonical, &[json!({}), json!({"value": 1})]);
            }
        }
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
        ] {
            assert_projection(
                &json!({"$schema": dialect, "type": "object", "dependentSchemas": {
                    "trigger": {"$ref": "#/missing"},
                }}),
                &[json!({}), json!({"trigger": true})],
            );
        }
    }

    #[test]
    fn if_without_branches_still_supplies_unevaluated_annotations() {
        for dialect in [
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ] {
            let conditional = json!({"if": {"patternProperties": {"foo": {"type": "string"}}}});
            for mut canonical in [conditional.clone(), json!({"allOf": [conditional]})] {
                canonical["$schema"] = json!(dialect);
                canonical["type"] = json!("object");
                canonical["unevaluatedProperties"] = json!(false);
                assert_projection(
                    &canonical,
                    &[
                        json!({}),
                        json!({"foo": "a"}),
                        json!({"foo": 1}),
                        json!({"extra": true}),
                    ],
                );
            }
            let invalid = json!({
                "$schema": dialect, "type": "object",
                "if": {"$ref": "#/missing"}, "unevaluatedProperties": false,
            });
            assert!(build_validator(&invalid).is_err());
            assert!(compile_business_resources(&invalid).is_err());
        }
    }

    #[test]
    fn dynamic_paths_keep_overrides_declared_in_resource_definitions() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.test/lists/main", "type": "object",
            "if": {"properties": {"kind": {"const": "numbers"}}, "required": ["kind"]},
            "then": {"$ref": "numbers"}, "else": {"$ref": "strings"},
            "$defs": {
                "base": {"$id": "base", "properties": {
                    "list": {"items": {"$dynamicRef": "#item"}},
                }, "$defs": {"default": {"$dynamicAnchor": "item"}}},
                "numbers": {"$id": "numbers", "$ref": "base", "$defs": {
                    "item": {"$dynamicAnchor": "item", "type": "number"},
                }},
                "strings": {"$id": "strings", "$ref": "base", "$defs": {
                    "item": {"$dynamicAnchor": "item", "type": "string"},
                }},
            },
        });
        assert_projection(
            &canonical,
            &[
                json!({"kind": "numbers", "list": [1.1]}),
                json!({"kind": "numbers", "list": ["bad"]}),
                json!({"kind": "strings", "list": ["yes"]}),
                json!({"kind": "strings", "list": [1.1]}),
            ],
        );
    }

    #[test]
    fn unused_external_declarations_do_not_authorize_host_resource_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unused.json");
        let marker = "inactive-resource-still-needs-owner-capture";
        std::fs::write(&path, json!({"x-private-marker": marker}).to_string()).unwrap();
        let canonical = json!({
            "type": "object", "properties": {"value": {"type": "integer"}},
            "$defs": {"unused": {"$ref": format!("file://{}", path.display())}},
        });
        // Registry discovery can resolve declarations before evaluation. An
        // inactive reference still cannot confer permission to retrieve bytes.
        let error = compile_business_resources(&canonical)
            .err()
            .expect("uncaptured declaration must not read the host");
        assert!(error.to_string().contains("does not fetch resources"));
        assert!(!error.to_string().contains(marker));
    }

    #[test]
    fn owner_bundled_inactive_resources_are_not_executed_or_disclosed() {
        let uri = "file:///owner-captured/unused.json";
        let marker = "inactive-owner-bytes-do-not-enter-model-definition";
        let canonical = json!({
            "type": "object", "properties": {"value": {"type": "integer"}},
            "$defs": {
                "unused": {"$ref": uri},
                "captured": {"$id": uri, "required": ["not-required-by-root"],
                    "$ref": "#/missing", "x-owner-marker": marker},
            },
        });
        let original = build_validator(&canonical).unwrap();
        let business = compile_business_resources(&canonical).unwrap();
        let model_schema = envelope_for(&business);
        assert!(!model_schema.to_string().contains(marker));
        let compiled = build_validator(&model_schema).unwrap();
        for value in [json!({}), json!({"value": 1}), json!({"value": "bad"})] {
            assert_eq!(
                compiled.is_valid(&json!({"arguments": value})),
                original.is_valid(&value),
            );
        }
    }

    #[test]
    fn references_into_declarations_and_dynamic_anchors_remain_executable() {
        let canonical = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object",
            "properties": {"child": {"$dynamicRef": "#node"}},
            "$defs": {
                "unused": {"$ref": "#/missing"},
                "node": {"$dynamicAnchor": "node", "type": "object",
                    "properties": {"value": {"type": "integer"}},
                    "additionalProperties": false,
                },
            },
        });
        assert_projection(
            &canonical,
            &[
                json!({}),
                json!({"child": {"value": 1}}),
                json!({"child": {"value": "bad"}}),
            ],
        );
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ] {
            // `dependencies` is still executed by the current validator in
            // modern dialects, and an explicit ref can use an inactive `then`.
            let canonical = json!({
                "$schema": dialect, "type": "object",
                "dependencies": {"trigger": {"$ref": "#/then"}},
                "then": {"required": ["needed"]},
                "definitions": {"unused": {"$ref": "#/missing"}},
            });
            assert_projection(
                &canonical,
                &[
                    json!({}),
                    json!({"trigger": true}),
                    json!({"trigger": true, "needed": true}),
                ],
            );
        }
    }

    #[test]
    fn malformed_reference_returns_schema_error() {
        let canonical = json!({"type": "object", "$ref": "#/missing"});
        assert!(matches!(
            compile_business_resources(&canonical),
            Err(SchemaError::InvalidSchema(_))
        ));
        let ignored_invalid_sibling = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object", "$ref": "#/definitions/body",
            "properties": [], "definitions": {"body": {"type": "object"}},
        });
        assert!(matches!(
            compile_business_resources(&ignored_invalid_sibling),
            Err(SchemaError::InvalidSchema(_)),
        ));
    }

    #[test]
    fn declaration_validation_checks_structure_without_retrieving_references() {
        for dialect in [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ] {
            let mut schema = json!({
                "$schema": dialect, "type": "object",
                "$ref": "http://127.0.0.1:1/never-fetched.json",
            });
            validate_declaration(&schema).expect("structure does not retrieve external references");
            schema["properties"] = json!([]);
            assert!(validate_declaration(&schema).is_err());
        }
    }
}
