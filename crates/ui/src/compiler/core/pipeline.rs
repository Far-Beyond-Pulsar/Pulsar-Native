//! # Compilation Pipeline
//!
//! High-level orchestration of the multi-phase compilation process.
//!
//! This module provides types and functions for managing the compilation pipeline,
//! tracking progress through phases, and handling intermediate representations.

use crate::graph::GraphDescription;
use std::collections::HashMap;

/// Compilation phase identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompilationPhase {
    /// Initial graph loading and validation
    Loading,
    /// Sub-graph expansion (inlining macros)
    Expansion,
    /// Metadata extraction from pulsar_std
    MetadataLoading,
    /// Data flow dependency analysis
    DataFlowAnalysis,
    /// Execution flow routing analysis
    ExecutionFlowAnalysis,
    /// Rust code generation
    CodeGeneration,
    /// Completed successfully
    Complete,
}

impl CompilationPhase {
    /// Get human-readable phase name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Loading => "Loading",
            Self::Expansion => "Sub-Graph Expansion",
            Self::MetadataLoading => "Metadata Loading",
            Self::DataFlowAnalysis => "Data Flow Analysis",
            Self::ExecutionFlowAnalysis => "Execution Flow Analysis",
            Self::CodeGeneration => "Code Generation",
            Self::Complete => "Complete",
        }
    }

    /// Get phase description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Loading => "Loading and validating input graph",
            Self::Expansion => "Expanding sub-graph instances inline",
            Self::MetadataLoading => "Loading node definitions from pulsar_std",
            Self::DataFlowAnalysis => "Building data dependency graph and evaluation order",
            Self::ExecutionFlowAnalysis => "Mapping execution connections between nodes",
            Self::CodeGeneration => "Generating Rust source code",
            Self::Complete => "Compilation completed successfully",
        }
    }

    /// Get completion percentage (0-100)
    pub fn progress_percent(&self) -> u8 {
        match self {
            Self::Loading => 0,
            Self::Expansion => 15,
            Self::MetadataLoading => 30,
            Self::DataFlowAnalysis => 50,
            Self::ExecutionFlowAnalysis => 70,
            Self::CodeGeneration => 85,
            Self::Complete => 100,
        }
    }
}

/// Compilation context holding intermediate data
///
/// Tracks the state of compilation as it progresses through phases.
/// Can be used for progress reporting, debugging, or incremental compilation.
pub struct CompilationContext {
    /// Current phase
    pub phase: CompilationPhase,
    
    /// Input graph
    pub graph: GraphDescription,
    
    /// Graph after expansion (if applicable)
    pub expanded_graph: Option<GraphDescription>,
    
    /// Any warnings generated during compilation
    pub warnings: Vec<String>,
    
    /// Statistics about the compilation
    pub stats: CompilationStats,
}

/// Compilation statistics
#[derive(Debug, Default, Clone)]
pub struct CompilationStats {
    /// Number of nodes in input graph
    pub input_nodes: usize,
    
    /// Number of connections in input graph
    pub input_connections: usize,
    
    /// Number of nodes after expansion
    pub expanded_nodes: usize,
    
    /// Number of sub-graphs expanded
    pub subgraphs_expanded: usize,
    
    /// Number of pure nodes
    pub pure_nodes: usize,
    
    /// Number of function nodes
    pub function_nodes: usize,
    
    /// Number of control flow nodes
    pub control_flow_nodes: usize,
    
    /// Number of event nodes
    pub event_nodes: usize,
    
    /// Generated code size in bytes
    pub generated_code_size: usize,
    
    /// Total compilation time in milliseconds
    pub compilation_time_ms: u64,
}

impl CompilationStats {
    /// Create statistics from a graph
    pub fn from_graph(graph: &GraphDescription) -> Self {
        Self {
            input_nodes: graph.nodes.len(),
            input_connections: graph.connections.len(),
            ..Default::default()
        }
    }

    /// Pretty-print statistics
    pub fn print_summary(&self) {
        tracing::info!("\n=== Compilation Statistics ===");
        tracing::info!("Input Graph:");
        tracing::info!("  Nodes:       {}", self.input_nodes);
        tracing::info!("  Connections: {}", self.input_connections);
        
        if self.subgraphs_expanded > 0 {
            tracing::info!("\nExpansion:");
            tracing::info!("  Sub-graphs expanded: {}", self.subgraphs_expanded);
            tracing::info!("  Nodes after expansion: {}", self.expanded_nodes);
        }
        
        tracing::info!("\nNode Types:");
        tracing::info!("  Pure nodes:         {}", self.pure_nodes);
        tracing::info!("  Function nodes:     {}", self.function_nodes);
        tracing::info!("  Control flow nodes: {}", self.control_flow_nodes);
        tracing::info!("  Event nodes:        {}", self.event_nodes);
        
        tracing::info!("\nOutput:");
        tracing::info!("  Generated code size: {} bytes", self.generated_code_size);
        
        if self.compilation_time_ms > 0 {
            tracing::info!("  Compilation time: {} ms", self.compilation_time_ms);
        }
        
        tracing::info!("==============================\n");
    }
}

impl CompilationContext {
    /// Create a new compilation context
    pub fn new(graph: GraphDescription) -> Self {
        let stats = CompilationStats::from_graph(&graph);
        
        Self {
            phase: CompilationPhase::Loading,
            graph,
            expanded_graph: None,
            warnings: Vec::new(),
            stats,
        }
    }

    /// Advance to next phase
    pub fn advance_phase(&mut self, phase: CompilationPhase) {
        self.phase = phase;
        tracing::info!("[COMPILER] Phase {}: {} ({}%)",
                 phase.name(),
                 phase.description(),
                 phase.progress_percent());
    }

    /// Add a warning message
    pub fn add_warning(&mut self, warning: String) {
        tracing::warn!("[COMPILER] Warning: {}", warning);
        self.warnings.push(warning);
    }

    /// Get the graph to compile (expanded if available, otherwise original)
    pub fn get_graph(&self) -> &GraphDescription {
        self.expanded_graph.as_ref().unwrap_or(&self.graph)
    }
}

/// Compilation options
#[derive(Debug, Clone)]
pub struct CompilationOptions {
    /// Enable verbose logging
    pub verbose: bool,
    
    /// Generate debug comments in output
    pub debug_comments: bool,
    
    /// Optimize generated code
    pub optimize: bool,
    
    /// Include source map information
    pub source_maps: bool,
    
    /// Maximum sub-graph nesting depth
    pub max_subgraph_depth: usize,
    
    /// Class variables (name -> type)
    pub variables: HashMap<String, String>,
}

impl Default for CompilationOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            debug_comments: true,
            optimize: false,
            source_maps: false,
            max_subgraph_depth: 100,
            variables: HashMap::new(),
        }
    }
}

impl CompilationOptions {
    /// Create options with verbose logging enabled
    pub fn verbose() -> Self {
        Self {
            verbose: true,
            ..Default::default()
        }
    }

    /// Create options for production builds
    pub fn production() -> Self {
        Self {
            verbose: false,
            debug_comments: false,
            optimize: true,
            source_maps: false,
            ..Default::default()
        }
    }

    /// Create options for development/debugging
    pub fn development() -> Self {
        Self {
            verbose: true,
            debug_comments: true,
            optimize: false,
            source_maps: true,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_ordering() {
        use CompilationPhase::*;
        
        assert!(Loading < Expansion);
        assert!(Expansion < MetadataLoading);
        assert!(MetadataLoading < DataFlowAnalysis);
        assert!(DataFlowAnalysis < ExecutionFlowAnalysis);
        assert!(ExecutionFlowAnalysis < CodeGeneration);
        assert!(CodeGeneration < Complete);
    }

    #[test]
    fn test_phase_progress() {
        use CompilationPhase::*;
        
        assert_eq!(Loading.progress_percent(), 0);
        assert_eq!(Complete.progress_percent(), 100);
        
        // Progress should increase monotonically
        let phases = vec![
            Loading,
            Expansion,
            MetadataLoading,
            DataFlowAnalysis,
            ExecutionFlowAnalysis,
            CodeGeneration,
            Complete,
        ];
        
        for window in phases.windows(2) {
            assert!(window[0].progress_percent() < window[1].progress_percent());
        }
    }

    #[test]
    fn test_compilation_options() {
        let dev = CompilationOptions::development();
        assert!(dev.verbose);
        assert!(dev.debug_comments);
        assert!(!dev.optimize);
        
        let prod = CompilationOptions::production();
        assert!(!prod.verbose);
        assert!(!prod.debug_comments);
        assert!(prod.optimize);
    }
}
