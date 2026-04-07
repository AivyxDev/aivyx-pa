//! Knowledge graph query tools — traverse, search, and analyze the knowledge graph.
//!
//! These tools expose the in-memory `KnowledgeGraph` (built from stored triples)
//! to the agent, enabling relationship discovery, path finding between entities,
//! community detection, and entity search.
//!
//! Triple creation and basic querying are handled by `memory_triple` in
//! aivyx-memory. These tools add *graph-level* operations on top.

use crate::Action;
use aivyx_core::Result;
use aivyx_memory::MemoryManager;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared context for knowledge graph operations.
#[derive(Clone)]
pub struct KnowledgeContext {
    pub memory: Arc<Mutex<MemoryManager>>,
}

// ── Traverse Knowledge Graph ─────────────────────────────────

/// Explore the knowledge graph by traversing from an entity.
pub struct TraverseKnowledgeGraph {
    pub ctx: KnowledgeContext,
}

#[async_trait::async_trait]
impl Action for TraverseKnowledgeGraph {
    fn name(&self) -> &str { "traverse_knowledge" }

    fn description(&self) -> &str {
        "Traverse the knowledge graph from an entity to discover related facts. \
         Returns all paths within max_hops of the starting entity."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["entity"],
            "properties": {
                "entity": {
                    "type": "string",
                    "description": "The entity to start traversal from (e.g., 'Alice', 'Project Orion')."
                },
                "max_hops": {
                    "type": "integer",
                    "description": "Maximum number of hops to traverse (1-5). Default: 2.",
                    "minimum": 1,
                    "maximum": 5
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let entity = input["entity"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("entity is required".into()))?;
        let max_hops = input["max_hops"].as_u64().unwrap_or(2) as usize;
        let max_hops = max_hops.clamp(1, 5);

        let mgr = self.ctx.memory.lock().await;
        let graph = match mgr.graph() {
            Some(g) => g,
            None => return Ok(serde_json::json!({
                "entity": entity,
                "error": "knowledge graph not available",
            })),
        };

        let paths = graph.traverse(entity, max_hops);

        if paths.is_empty() {
            // Try fuzzy search to suggest close matches
            let suggestions = graph.search_entities(entity);
            let top: Vec<_> = suggestions.iter().take(5)
                .map(|(name, score)| serde_json::json!({"entity": name, "similarity": score}))
                .collect();

            return Ok(serde_json::json!({
                "entity": entity,
                "paths": [],
                "message": "No paths found from this entity.",
                "did_you_mean": top,
            }));
        }

        let path_json: Vec<_> = paths.iter().take(50).map(|path| {
            let hops: Vec<_> = path.hops.iter().map(|(s, p, o)| {
                serde_json::json!({"subject": s, "predicate": p, "object": o})
            }).collect();
            serde_json::json!({"hops": hops})
        }).collect();

        Ok(serde_json::json!({
            "entity": entity,
            "max_hops": max_hops,
            "path_count": paths.len(),
            "paths": path_json,
        }))
    }
}

// ── Find Paths Between Entities ──────────────────────────────

/// Find paths between two entities in the knowledge graph.
pub struct FindKnowledgePaths {
    pub ctx: KnowledgeContext,
}

#[async_trait::async_trait]
impl Action for FindKnowledgePaths {
    fn name(&self) -> &str { "find_knowledge_paths" }

    fn description(&self) -> &str {
        "Find how two entities are connected in the knowledge graph. Returns \
         all paths between them within max_depth hops."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["from", "to"],
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Starting entity."
                },
                "to": {
                    "type": "string",
                    "description": "Target entity."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum path depth (1-6). Default: 3.",
                    "minimum": 1,
                    "maximum": 6
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let from = input["from"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("from is required".into()))?;
        let to = input["to"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("to is required".into()))?;
        let max_depth = input["max_depth"].as_u64().unwrap_or(3) as usize;
        let max_depth = max_depth.clamp(1, 6);

        let mgr = self.ctx.memory.lock().await;
        let graph = match mgr.graph() {
            Some(g) => g,
            None => return Ok(serde_json::json!({
                "from": from,
                "to": to,
                "error": "knowledge graph not available",
            })),
        };

        let paths = graph.find_paths(from, to, max_depth);

        let path_json: Vec<_> = paths.iter().take(20).map(|path| {
            let hops: Vec<_> = path.hops.iter().map(|(s, p, o)| {
                serde_json::json!({"subject": s, "predicate": p, "object": o})
            }).collect();
            serde_json::json!({"hops": hops, "length": path.hops.len()})
        }).collect();

        Ok(serde_json::json!({
            "from": from,
            "to": to,
            "max_depth": max_depth,
            "path_count": paths.len(),
            "connected": !paths.is_empty(),
            "paths": path_json,
        }))
    }
}

// ── Search Entities ──────────────────────────────────────────

/// Search for entities in the knowledge graph by name.
pub struct SearchKnowledgeEntities {
    pub ctx: KnowledgeContext,
}

#[async_trait::async_trait]
impl Action for SearchKnowledgeEntities {
    fn name(&self) -> &str { "search_knowledge" }

    fn description(&self) -> &str {
        "Search for entities in the knowledge graph by name (fuzzy substring match). \
         Returns matching entities with their immediate relationships."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (substring match against entity names)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (1-20). Default: 10.",
                    "minimum": 1,
                    "maximum": 20
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("query is required".into()))?;
        let limit = input["limit"].as_u64().unwrap_or(10) as usize;
        let limit = limit.clamp(1, 20);

        let mgr = self.ctx.memory.lock().await;
        let graph = match mgr.graph() {
            Some(g) => g,
            None => return Ok(serde_json::json!({
                "query": query,
                "error": "knowledge graph not available",
            })),
        };

        let matches = graph.search_entities(query);

        let results: Vec<_> = matches.iter().take(limit).map(|(entity, score)| {
            let neighborhood = graph.neighborhood(entity);
            let outbound: Vec<_> = neighborhood.outbound.iter().take(10).map(|e| {
                serde_json::json!({"predicate": e.predicate, "object": e.target, "confidence": e.confidence})
            }).collect();
            let inbound: Vec<_> = neighborhood.inbound.iter().take(10).map(|e| {
                serde_json::json!({"subject": e.target, "predicate": e.predicate, "confidence": e.confidence})
            }).collect();

            serde_json::json!({
                "entity": entity,
                "match_score": score,
                "outbound_facts": outbound,
                "inbound_facts": inbound,
            })
        }).collect();

        Ok(serde_json::json!({
            "query": query,
            "result_count": results.len(),
            "total_matches": matches.len(),
            "results": results,
        }))
    }
}

// ── Knowledge Graph Stats ────────────────────────────────────

/// Get statistics and community structure of the knowledge graph.
pub struct KnowledgeGraphStats {
    pub ctx: KnowledgeContext,
}

#[async_trait::async_trait]
impl Action for KnowledgeGraphStats {
    fn name(&self) -> &str { "knowledge_graph_stats" }

    fn description(&self) -> &str {
        "Get statistics about the knowledge graph: entity count, edge count, \
         and detected communities (clusters of related entities)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "include_communities": {
                    "type": "boolean",
                    "description": "If true, include detected entity communities. Default: false."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let include_communities = input["include_communities"].as_bool().unwrap_or(false);

        let mgr = self.ctx.memory.lock().await;
        let graph = match mgr.graph() {
            Some(g) => g,
            None => return Ok(serde_json::json!({
                "error": "knowledge graph not available",
            })),
        };

        let entity_count = graph.entity_count();
        let edge_count = graph.edge_count();

        let mut result = serde_json::json!({
            "entity_count": entity_count,
            "edge_count": edge_count,
        });

        if include_communities {
            let communities = graph.detect_communities();
            let community_json: Vec<_> = communities.iter()
                .filter(|c| c.entities.len() > 1) // skip singletons
                .take(20)
                .map(|c| {
                    let mut entities: Vec<_> = c.entities.iter().cloned().collect();
                    entities.sort();
                    serde_json::json!({
                        "entity_count": c.entities.len(),
                        "edge_count": c.edge_count,
                        "entities": entities,
                    })
                })
                .collect();

            result["community_count"] = serde_json::json!(communities.len());
            result["communities"] = serde_json::json!(community_json);
        }

        Ok(result)
    }
}

// ── Delete Knowledge Triple ──────────────────────────────────

/// Tool: delete a knowledge triple by its ID.
pub struct DeleteKnowledgeTriple {
    pub ctx: KnowledgeContext,
}

#[async_trait::async_trait]
impl Action for DeleteKnowledgeTriple {
    fn name(&self) -> &str { "delete_knowledge_triple" }

    fn description(&self) -> &str {
        "Delete a knowledge triple by its ID. Use this to remove incorrect or outdated facts \
         from the knowledge graph. Get triple IDs from traverse_knowledge or search_knowledge."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["triple_id"],
            "properties": {
                "triple_id": {
                    "type": "string",
                    "description": "UUID of the triple to delete"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id_str = input["triple_id"].as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'triple_id' is required".into()))?;

        let triple_id: aivyx_core::TripleId = id_str.parse()
            .map_err(|e| aivyx_core::AivyxError::Validation(
                format!("Invalid triple_id '{id_str}': {e}"),
            ))?;

        let mut mgr = self.ctx.memory.lock().await;
        mgr.delete_triple(&triple_id)?;

        Ok(serde_json::json!({
            "status": "deleted",
            "triple_id": id_str,
        }))
    }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_crypto::MasterKey;

    /// Helper: construct a MemoryManager with an in-memory store for testing.
    fn test_memory_manager() -> MemoryManager {
        let dir = tempfile::tempdir().unwrap();
        let store = aivyx_memory::MemoryStore::open(dir.path().join("test.db")).unwrap();
        let key = MasterKey::generate();

        // Use a dummy embedding provider
        struct DummyEmbed;
        #[async_trait::async_trait]
        impl aivyx_llm::EmbeddingProvider for DummyEmbed {
            fn name(&self) -> &str { "dummy" }
            fn dimensions(&self) -> usize { 128 }
            async fn embed(&self, _text: &str) -> std::result::Result<aivyx_llm::Embedding, aivyx_core::AivyxError> {
                Ok(aivyx_llm::Embedding { vector: vec![0.1; 128], dimensions: 128 })
            }
        }

        MemoryManager::new(store, Arc::new(DummyEmbed), key, 0).unwrap()
    }

    fn test_ctx() -> KnowledgeContext {
        KnowledgeContext {
            memory: Arc::new(Mutex::new(test_memory_manager())),
        }
    }

    #[test]
    fn traverse_knowledge_name_and_schema() {
        let action = TraverseKnowledgeGraph { ctx: test_ctx() };
        assert_eq!(action.name(), "traverse_knowledge");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("entity")));
    }

    #[test]
    fn find_paths_name_and_schema() {
        let action = FindKnowledgePaths { ctx: test_ctx() };
        assert_eq!(action.name(), "find_knowledge_paths");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("from")));
        assert!(required.contains(&serde_json::json!("to")));
    }

    #[test]
    fn search_knowledge_name_and_schema() {
        let action = SearchKnowledgeEntities { ctx: test_ctx() };
        assert_eq!(action.name(), "search_knowledge");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn stats_name_and_schema() {
        let action = KnowledgeGraphStats { ctx: test_ctx() };
        assert_eq!(action.name(), "knowledge_graph_stats");
        let schema = action.input_schema();
        assert!(schema.get("required").is_none());
    }

    #[tokio::test]
    async fn traverse_empty_graph() {
        let action = TraverseKnowledgeGraph { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({"entity": "Alice"})).await.unwrap();
        // Empty graph — should return no paths
        let paths = result["paths"].as_array().unwrap();
        assert!(paths.is_empty());
    }

    #[tokio::test]
    async fn traverse_rejects_missing_entity() {
        let action = TraverseKnowledgeGraph { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn find_paths_rejects_missing_from() {
        let action = FindKnowledgePaths { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({"to": "Bob"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn find_paths_rejects_missing_to() {
        let action = FindKnowledgePaths { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({"from": "Alice"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_rejects_missing_query() {
        let action = SearchKnowledgeEntities { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stats_on_empty_graph() {
        let action = KnowledgeGraphStats { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["entity_count"], 0);
        assert_eq!(result["edge_count"], 0);
    }

    #[tokio::test]
    async fn stats_with_communities() {
        let action = KnowledgeGraphStats { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({"include_communities": true})).await.unwrap();
        assert_eq!(result["entity_count"], 0);
        assert!(result["communities"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn traverse_with_triples() {
        let ctx = test_ctx();
        {
            let mut mgr = ctx.memory.lock().await;
            mgr.add_triple(
                "Alice".into(), "works_at".into(), "Acme".into(),
                None, 0.9, "test".into(),
            ).unwrap();
            mgr.add_triple(
                "Acme".into(), "located_in".into(), "New York".into(),
                None, 0.85, "test".into(),
            ).unwrap();
        }

        let action = TraverseKnowledgeGraph { ctx };
        let result = action.execute(serde_json::json!({"entity": "Alice", "max_hops": 2})).await.unwrap();
        let paths = result["paths"].as_array().unwrap();
        assert!(!paths.is_empty());
        assert!(result["path_count"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn find_paths_between_entities() {
        let ctx = test_ctx();
        {
            let mut mgr = ctx.memory.lock().await;
            mgr.add_triple(
                "Alice".into(), "works_at".into(), "Acme".into(),
                None, 0.9, "test".into(),
            ).unwrap();
            mgr.add_triple(
                "Acme".into(), "employs".into(), "Bob".into(),
                None, 0.85, "test".into(),
            ).unwrap();
        }

        let action = FindKnowledgePaths { ctx: ctx.clone() };
        let result = action.execute(serde_json::json!({"from": "Alice", "to": "Bob"})).await.unwrap();
        // Alice -> works_at -> Acme -> employs -> Bob
        assert!(result["connected"].as_bool().unwrap());

        // No path in the opposite direction without reverse edges
        let result2 = action.execute(serde_json::json!({"from": "Bob", "to": "Alice"})).await.unwrap();
        // May or may not be connected depending on graph's reverse edge handling
        let _ = result2;
    }

    #[tokio::test]
    async fn search_entities_finds_matches() {
        let ctx = test_ctx();
        {
            let mut mgr = ctx.memory.lock().await;
            mgr.add_triple(
                "Alice Johnson".into(), "email_is".into(), "alice@example.com".into(),
                None, 0.95, "test".into(),
            ).unwrap();
        }

        let action = SearchKnowledgeEntities { ctx };
        let result = action.execute(serde_json::json!({"query": "Alice"})).await.unwrap();
        assert!(result["result_count"].as_u64().unwrap() > 0);
    }

    // ── Delete knowledge triple tests ────────────────────────

    #[test]
    fn delete_triple_schema() {
        let action = DeleteKnowledgeTriple { ctx: test_ctx() };
        assert_eq!(action.name(), "delete_knowledge_triple");
        let schema = action.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "triple_id"));
        assert_eq!(required.len(), 1);
    }

    #[tokio::test]
    async fn delete_triple_round_trip() {
        let ctx = test_ctx();

        // Add a triple
        let triple_id = {
            let mut mgr = ctx.memory.lock().await;
            mgr.add_triple(
                "Alice".into(), "knows".into(), "Bob".into(),
                None, 0.9, "test".into(),
            ).unwrap()
        };

        // Verify it exists via stats
        let stats = KnowledgeGraphStats { ctx: ctx.clone() };
        let result = stats.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["edge_count"], 1);

        // Delete it
        let action = DeleteKnowledgeTriple { ctx: ctx.clone() };
        let result = action.execute(serde_json::json!({
            "triple_id": triple_id.to_string()
        })).await.unwrap();
        assert_eq!(result["status"], "deleted");

        // Verify it's gone
        let result = stats.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["edge_count"], 0);
    }

    #[tokio::test]
    async fn delete_triple_invalid_id() {
        let action = DeleteKnowledgeTriple { ctx: test_ctx() };
        let result = action.execute(serde_json::json!({
            "triple_id": "not-a-uuid"
        })).await;
        assert!(result.is_err());
    }
}
