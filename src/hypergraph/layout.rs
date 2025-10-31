//! Force-directed layout algorithm for 3D hypergraph visualization
//!
//! Based on Fruchterman-Reingold algorithm adapted for hypergraphs

use crate::hypergraph::{Hypergraph, NodeId};
use rand::Rng;
use std::collections::HashMap;

/// 3D position vector (compatible with Bevy)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalize(self) -> Self {
        let len = self.length();
        if len > 0.0 {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            Vec3::ZERO
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
            z: self.z * scalar,
        }
    }
}

impl std::ops::Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, scalar: f32) -> Self {
        Self {
            x: self.x / scalar,
            y: self.y / scalar,
            z: self.z / scalar,
        }
    }
}

impl std::ops::AddAssign for Vec3 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}

impl std::ops::SubAssign for Vec3 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}

/// Configuration for layout algorithm
pub struct LayoutConfig {
    /// Number of iterations (more = better layout, slower)
    pub iterations: usize,

    /// Repulsion strength (higher = nodes push apart more)
    pub repulsion_strength: f32,

    /// Attraction strength (higher = hyperedge nodes cluster more)
    pub attraction_strength: f32,

    /// Initial random spread range
    pub initial_spread: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            iterations: 200,
            repulsion_strength: 100.0,
            attraction_strength: 0.15,
            initial_spread: 50.0,
        }
    }
}

/// Computes 3D positions for all nodes using force-directed layout
///
/// # Arguments
/// * `hg` - The hypergraph to layout
/// * `config` - Layout algorithm parameters
///
/// # Returns
/// HashMap mapping NodeId → Vec3 position
pub fn compute_layout(hg: &Hypergraph, config: &LayoutConfig) -> HashMap<NodeId, Vec3> {
    let node_count = hg.count_nodes();

    if node_count == 0 {
        return HashMap::new();
    }

    // Initialize positions randomly
    let mut positions = initialize_positions(hg, config.initial_spread);

    // Collect node IDs for iteration
    let node_ids: Vec<NodeId> = positions.keys().copied().collect();

    println!("Starting layout computation: {} nodes, {} iterations",
        node_count, config.iterations);

    // Force-directed iterations
    for iteration in 0..config.iterations {
        // Temperature: decreases linearly from 1.0 to 0.0
        let temperature = 1.0 - (iteration as f32 / config.iterations as f32);

        // Print progress every 20 iterations
        if iteration % 20 == 0 {
            println!("  Iteration {}/{} (temp: {:.2})",
                iteration, config.iterations, temperature);
        }

        // Apply repulsion between all node pairs
        apply_repulsion(&mut positions, &node_ids, config.repulsion_strength, temperature);

        // Apply attraction within hyperedges
        apply_attraction_hyperedges(&mut positions, hg, config.attraction_strength, temperature);
    }

    println!("Layout computation complete!");

    positions
}

/// Initialize node positions randomly in 3D space
fn initialize_positions(hg: &Hypergraph, spread: f32) -> HashMap<NodeId, Vec3> {
    let mut rng = rand::thread_rng();
    let mut positions = HashMap::new();

    // Get all node IDs (0..node_count is stable)
    for i in 0..hg.count_nodes() {
        let node_id = NodeId(i);
        let pos = Vec3::new(
            rng.gen_range(-spread..spread),
            rng.gen_range(-spread..spread),
            rng.gen_range(-spread..spread),
        );
        positions.insert(node_id, pos);
    }

    positions
}

/// Apply repulsion force between all node pairs
///
/// Nodes push apart like electric charges: force ∝ 1/distance²
fn apply_repulsion(
    positions: &mut HashMap<NodeId, Vec3>,
    node_ids: &[NodeId],
    strength: f32,
    temperature: f32,
) {
    let n = node_ids.len();

    for i in 0..n {
        for j in (i + 1)..n {
            let id_i = node_ids[i];
            let id_j = node_ids[j];

            let pos_i = positions[&id_i];
            let pos_j = positions[&id_j];

            // Vector from i to j
            let diff = pos_j - pos_i;
            let dist = diff.length().max(0.1); // Avoid division by zero

            // Repulsion force: inversely proportional to distance²
            let force_magnitude = strength / (dist * dist);
            let force = diff.normalize() * force_magnitude * temperature;

            // Apply force (i pushes j away, j pushes i away)
            *positions.get_mut(&id_i).unwrap() -= force;
            *positions.get_mut(&id_j).unwrap() += force;
        }
    }
}

/// Apply attraction force within hyperedges
///
/// Nodes in the same hyperedge are pulled toward the hyperedge centroid
fn apply_attraction_hyperedges(
    positions: &mut HashMap<NodeId, Vec3>,
    hg: &Hypergraph,
    strength: f32,
    temperature: f32,
) {
    // Iterate over all hyperedges
    for edge_idx in 0..hg.count_hyperedges() {
        let edge_id = crate::hypergraph::HyperedgeId(edge_idx);

        // Get hyperedge
        let edge = match hg.get_hyperedge(edge_id) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Collect all node IDs in this hyperedge
        let all_nodes: Vec<NodeId> = edge.sources.iter()
            .chain(edge.targets.iter())
            .copied()
            .collect();

        if all_nodes.is_empty() {
            continue;
        }

        // Compute centroid of this hyperedge
        let centroid = compute_centroid(&all_nodes, positions);

        // Pull each node toward centroid
        for &node_id in &all_nodes {
            if let Some(pos) = positions.get_mut(&node_id) {
                let to_centroid = centroid - *pos;
                *pos += to_centroid * strength * temperature;
            }
        }
    }
}

/// Compute centroid (average position) of a set of nodes
fn compute_centroid(node_ids: &[NodeId], positions: &HashMap<NodeId, Vec3>) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut count = 0;

    for &node_id in node_ids {
        if let Some(&pos) = positions.get(&node_id) {
            sum += pos;
            count += 1;
        }
    }

    if count > 0 {
        sum / count as f32
    } else {
        Vec3::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_operations() {
        let v1 = Vec3::new(1.0, 2.0, 3.0);
        let v2 = Vec3::new(4.0, 5.0, 6.0);

        let sum = v1 + v2;
        assert_eq!(sum, Vec3::new(5.0, 7.0, 9.0));

        let diff = v2 - v1;
        assert_eq!(diff, Vec3::new(3.0, 3.0, 3.0));
    }

    #[test]
    fn test_vec3_length() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(v.length(), 5.0);
    }

    #[test]
    fn test_initialize_positions() {
        use crate::hypergraph::Hypergraph;

        let mut hg = Hypergraph::new();
        // Add 3 test nodes
        for i in 0..3 {
            let node = crate::hypergraph::HyperNode {
                id: NodeId(i),
                name: format!("node{}", i),
                file_path: std::path::PathBuf::from("test.rs"),
                line_start: 0,
                line_end: 0,
                node_type: crate::hypergraph::types::NodeType::File {
                    path: std::path::PathBuf::from("test.rs")
                },
            };
            hg.add_node(node).unwrap();
        }

        let positions = initialize_positions(&hg, 10.0);
        assert_eq!(positions.len(), 3);
    }
}
