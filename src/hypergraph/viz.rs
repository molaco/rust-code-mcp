//! Bevy-based 3D visualization for hypergraphs

use crate::hypergraph::{Hypergraph, HyperedgeType, NodeId};
use crate::hypergraph::layout::{compute_layout, LayoutConfig, Vec3 as LayoutVec3};
use crate::hypergraph::viz_ui::{setup_legend, handle_keyboard_input, update_node_visibility};
use bevy::prelude::*;
use bevy::render::camera::Camera;
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};
use std::collections::HashMap;

/// Marker component for different node types
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NodeTypeMarker {
    Function,
    Struct,
    Trait,
    Enum,
    Other,
}

/// Component for edges tracking which node types they connect
#[derive(Component, Clone, Debug)]
pub struct EdgeMarker {
    pub source_types: Vec<NodeTypeMarker>,
    pub target_types: Vec<NodeTypeMarker>,
}

/// Resource tracking visibility state for each node type
#[derive(Resource)]
pub struct TypeVisibility {
    pub function: bool,
    pub struct_type: bool,
    pub trait_type: bool,
    pub enum_type: bool,
    pub other: bool,
}

impl Default for TypeVisibility {
    fn default() -> Self {
        Self {
            function: true,
            struct_type: true,
            trait_type: true,
            enum_type: true,
            other: true,
        }
    }
}

/// Component for legend UI
#[derive(Component)]
pub struct LegendUI;

/// Component for text labels that follow 3D nodes
#[derive(Component)]
pub struct NodeLabel {
    pub node_position: Vec3,
}

/// Launches interactive 3D visualization of the hypergraph
///
/// # Arguments
/// * `hypergraph` - The hypergraph to visualize
///
/// # Controls
/// * Left mouse drag: Rotate camera
/// * Right mouse drag: Pan camera
/// * Scroll wheel: Zoom
/// * ESC: Exit
pub fn visualize(hypergraph: Hypergraph) {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Hypergraph 3D Visualization".into(),
                resolution: (1920., 1080.).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(PanOrbitCameraPlugin)
        .insert_resource(HypergraphResource { hypergraph })
        .insert_resource(TypeVisibility::default())
        .add_systems(Startup, (setup_scene, setup_legend))
        .add_systems(Update, (handle_keyboard_input, update_node_visibility, update_node_labels))
        .run();
}

/// Resource holding the hypergraph
#[derive(Resource)]
struct HypergraphResource {
    hypergraph: Hypergraph,
}

/// Setup the 3D scene
fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    hg_res: Res<HypergraphResource>,
) {
    let hg = &hg_res.hypergraph;

    println!("\n=== Starting Bevy Visualization ===");
    println!("Nodes: {}", hg.count_nodes());
    println!("Hyperedges: {}", hg.count_hyperedges());

    // Compute layout
    let config = LayoutConfig::default();
    let layout = compute_layout(hg, &config);

    // Render nodes and labels
    spawn_nodes(&mut commands, &mut meshes, &mut materials, hg, &layout);

    // Render hyperedges
    spawn_hyperedges(&mut commands, &mut meshes, &mut materials, hg, &layout);

    // Setup camera
    setup_camera(&mut commands, hg);

    // Setup lighting
    setup_lighting(&mut commands);

    println!("=== Visualization Ready ===\n");
}

/// Spawn all node entities as spheres
fn spawn_nodes(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    hg: &Hypergraph,
    layout: &HashMap<NodeId, LayoutVec3>,
) {
    let sphere_mesh = meshes.add(Sphere::new(0.5));

    // Count node types for debugging
    let mut type_counts = std::collections::HashMap::new();

    for i in 0..hg.count_nodes() {
        let node_id = NodeId(i);

        let pos = layout.get(&node_id)
            .copied()
            .unwrap_or(LayoutVec3::ZERO);

        // Get node to determine color and extract name
        let (color, type_name, symbol_name) = if let Ok(node) = hg.get_node(node_id) {
            let (type_str, name) = match &node.node_type {
                crate::hypergraph::NodeType::File { path } => {
                    let file_name = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file");
                    ("File", file_name.to_string())
                },
                crate::hypergraph::NodeType::Symbol { symbol } => {
                    use crate::parser::SymbolKind;
                    let type_str = match symbol.kind {
                        SymbolKind::Function { .. } => "Function",
                        SymbolKind::Struct => "Struct",
                        SymbolKind::Trait => "Trait",
                        SymbolKind::Enum => "Enum",
                        _ => "Other",
                    };
                    (type_str, symbol.name.clone())
                }
            };
            *type_counts.entry(type_str).or_insert(0) += 1;
            (node_color(&node.node_type), type_str, name)
        } else {
            *type_counts.entry("Unknown").or_insert(0) += 1;
            (Color::srgb(0.5, 0.5, 0.5), "Unknown", "unknown".to_string())
        };

        let marker = match type_name {
            "Function" => NodeTypeMarker::Function,
            "Struct" => NodeTypeMarker::Struct,
            "Trait" => NodeTypeMarker::Trait,
            "Enum" => NodeTypeMarker::Enum,
            _ => NodeTypeMarker::Other,
        };

        let node_pos = Vec3::new(pos.x, pos.y, pos.z);

        // Spawn the sphere node
        commands.spawn((
            PbrBundle {
                mesh: sphere_mesh.clone(),
                material: materials.add(StandardMaterial {
                    base_color: color,
                    emissive: color.to_linear() * 0.3, // Add glow matching the node color
                    ..default()
                }),
                transform: Transform::from_translation(node_pos),
                ..default()
            },
            marker,
        ));

        // Spawn UI text label that will follow the node in screen space
        let label_text = format!("{}\n{}", symbol_name, type_name);
        commands.spawn((
            TextBundle {
                text: Text::from_section(
                    label_text,
                    TextStyle {
                        font_size: 14.0,
                        color: Color::WHITE,
                        ..default()
                    },
                ).with_justify(JustifyText::Center),
                style: Style {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                ..default()
            },
            NodeLabel {
                node_position: node_pos,
            },
            marker,
        ));
    }

    println!("Spawned {} node spheres:", hg.count_nodes());
    for (type_name, count) in type_counts.iter() {
        println!("  {} {}: {}",
            match *type_name {
                "Function" => "🟢",
                "Struct" => "🔵",
                "Trait" => "🟣",
                "Enum" => "🟡",
                "File" => "🟠",
                _ => "⚪",
            },
            type_name,
            count
        );
    }
}

/// Determine node color based on type
fn node_color(node_type: &crate::hypergraph::NodeType) -> Color {
    match node_type {
        crate::hypergraph::NodeType::File { .. } => {
            Color::srgb(1.0, 0.5, 0.0) // Bright orange for files
        }
        crate::hypergraph::NodeType::Symbol { symbol } => {
            use crate::parser::SymbolKind;
            match symbol.kind {
                SymbolKind::Function { .. } => Color::srgb(0.0, 1.0, 0.3), // Bright green for functions
                SymbolKind::Struct => Color::srgb(0.2, 0.5, 1.0), // Bright blue for structs
                SymbolKind::Trait => Color::srgb(1.0, 0.0, 1.0), // Bright magenta for traits
                SymbolKind::Enum => Color::srgb(1.0, 0.9, 0.0), // Bright yellow for enums
                _ => Color::srgb(0.9, 0.9, 0.9), // Light gray for others
            }
        }
    }
}

/// Get node type marker from node
fn get_node_type_marker(hg: &Hypergraph, node_id: NodeId) -> NodeTypeMarker {
    if let Ok(node) = hg.get_node(node_id) {
        match &node.node_type {
            crate::hypergraph::NodeType::File { .. } => NodeTypeMarker::Other,
            crate::hypergraph::NodeType::Symbol { symbol } => {
                use crate::parser::SymbolKind;
                match symbol.kind {
                    SymbolKind::Function { .. } => NodeTypeMarker::Function,
                    SymbolKind::Struct => NodeTypeMarker::Struct,
                    SymbolKind::Trait => NodeTypeMarker::Trait,
                    SymbolKind::Enum => NodeTypeMarker::Enum,
                    _ => NodeTypeMarker::Other,
                }
            }
        }
    } else {
        NodeTypeMarker::Other
    }
}

/// Spawn all hyperedge entities as lines
fn spawn_hyperedges(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    hg: &Hypergraph,
    layout: &HashMap<NodeId, LayoutVec3>,
) {
    let mut edge_count = 0;
    let mut edge_type_counts = std::collections::HashMap::new();

    for edge_idx in 0..hg.count_hyperedges() {
        let edge_id = crate::hypergraph::HyperedgeId(edge_idx);

        let edge = match hg.get_hyperedge(edge_id) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Get color based on edge type
        let color = hyperedge_color(&edge.edge_type);

        // Count edge types
        let type_name = match &edge.edge_type {
            HyperedgeType::ModuleContainment => "ModuleContainment",
            HyperedgeType::CallPattern => "CallPattern",
            HyperedgeType::TraitImpl { .. } => "TraitImpl",
            HyperedgeType::TypeComposition { .. } => "TypeComposition",
            HyperedgeType::ImportCluster => "ImportCluster",
        };

        // Collect source and target node types
        let source_types: Vec<NodeTypeMarker> = edge.sources
            .iter()
            .map(|&id| get_node_type_marker(hg, id))
            .collect();

        let target_types: Vec<NodeTypeMarker> = edge.targets
            .iter()
            .map(|&id| get_node_type_marker(hg, id))
            .collect();

        // Draw directed edges from each source to each target
        // This properly represents the hyperedge directionality
        for &source_id in &edge.sources {
            let source_pos = match layout.get(&source_id) {
                Some(pos) => pos,
                None => continue,
            };

            for &target_id in &edge.targets {
                let target_pos = match layout.get(&target_id) {
                    Some(pos) => pos,
                    None => continue,
                };

                spawn_line(
                    commands,
                    meshes,
                    materials,
                    source_pos,
                    target_pos,
                    color,
                    EdgeMarker {
                        source_types: source_types.clone(),
                        target_types: target_types.clone(),
                    },
                );
                edge_count += 1;
                *edge_type_counts.entry(type_name).or_insert(0) += 1;
            }
        }
    }

    println!("Spawned {} hyperedge lines:", edge_count);
    for (type_name, count) in edge_type_counts.iter() {
        println!("  {} {}: {}",
            match *type_name {
                "ModuleContainment" => "🔴",
                "CallPattern" => "🔵",
                "TraitImpl" => "🟣",
                "TypeComposition" => "🔵",
                "ImportCluster" => "🟡",
                _ => "⚪",
            },
            type_name,
            count
        );
    }
}

/// Determine hyperedge color based on type
fn hyperedge_color(edge_type: &HyperedgeType) -> Color {
    match edge_type {
        HyperedgeType::ModuleContainment => Color::srgba(1.0, 0.0, 0.0, 0.7), // Bright red
        HyperedgeType::CallPattern => Color::srgba(0.0, 0.6, 1.0, 0.7), // Bright cyan-blue
        HyperedgeType::TraitImpl { .. } => Color::srgba(1.0, 0.0, 0.8, 0.7), // Bright pink
        HyperedgeType::TypeComposition { .. } => Color::srgba(0.0, 1.0, 0.7, 0.7), // Bright teal
        HyperedgeType::ImportCluster => Color::srgba(1.0, 0.8, 0.0, 0.7), // Bright gold
    }
}

/// Spawn a line (cylinder) between two points with directional arrow
fn spawn_line(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    start: &LayoutVec3,
    end: &LayoutVec3,
    color: Color,
    edge_marker: EdgeMarker,
) {
    let start_vec = Vec3::new(start.x, start.y, start.z);
    let end_vec = Vec3::new(end.x, end.y, end.z);

    let direction = end_vec - start_vec;
    let length = direction.length();

    if length < 0.001 {
        return; // Skip zero-length edges
    }

    let normalized_direction = direction.normalize();
    let rotation = Quat::from_rotation_arc(Vec3::Y, normalized_direction);

    // Node radius is 0.5, so we need to account for that
    let node_radius = 0.5;
    let arrow_height = 1.2; // Make arrow bigger and more visible
    let arrow_radius = 0.25;

    // Position arrow outside target node sphere
    // Arrow base starts at node surface, arrow tip extends beyond
    let gap = 0.1; // Small gap between node and arrow for clarity
    let arrow_base_distance = node_radius + gap;
    let arrow_tip_distance = arrow_base_distance + arrow_height;

    // Shorten edge to stop before target node
    let cylinder_length = length - node_radius - arrow_base_distance - node_radius; // Account for both node radii
    let cylinder_start = start_vec + normalized_direction * node_radius;
    let cylinder_end = end_vec - normalized_direction * arrow_tip_distance;
    let cylinder_midpoint = (cylinder_start + cylinder_end) / 2.0;
    let actual_cylinder_length = (cylinder_end - cylinder_start).length();

    if actual_cylinder_length > 0.001 {
        // Create cylinder representing the edge body
        let cylinder_mesh = meshes.add(Cylinder::new(0.08, actual_cylinder_length));

        // Spawn the edge cylinder
        commands.spawn((
            PbrBundle {
                mesh: cylinder_mesh,
                material: materials.add(StandardMaterial {
                    base_color: color,
                    emissive: color.to_linear() * 0.4,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                }),
                transform: Transform {
                    translation: cylinder_midpoint,
                    rotation,
                    scale: Vec3::ONE,
                },
                ..default()
            },
            edge_marker.clone(),
        ));
    }

    // Create arrow head (cone) at the target end
    let cone_mesh = meshes.add(Cone {
        radius: arrow_radius,
        height: arrow_height,
    });

    // Position arrow head outside target node, pointing toward it
    // Cone's base (wide end) is at the back, tip points forward (along Y axis in local space)
    let arrow_position = end_vec - normalized_direction * (arrow_base_distance + arrow_height / 2.0);

    commands.spawn((
        PbrBundle {
            mesh: cone_mesh,
            material: materials.add(StandardMaterial {
                base_color: color,
                emissive: color.to_linear() * 0.5,
                alpha_mode: AlphaMode::Blend,
                ..default()
            }),
            transform: Transform {
                translation: arrow_position,
                rotation,
                scale: Vec3::ONE,
            },
            ..default()
        },
        edge_marker,
    ));
}

/// Setup camera with orbit controls
fn setup_camera(commands: &mut Commands, hg: &Hypergraph) {
    // Calculate good camera distance based on graph size
    let node_count = hg.count_nodes() as f32;
    let distance = (node_count.sqrt() * 20.0).max(80.0).min(200.0);

    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_xyz(0.0, distance * 0.5, distance)
                .looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
        PanOrbitCamera {
            radius: Some(distance),
            ..default()
        },
    ));

    println!("Camera distance: {:.1}", distance);
}

/// Setup scene lighting
fn setup_lighting(commands: &mut Commands) {
    // Ambient light
    commands.insert_resource(AmbientLight {
        color: Color::WHITE,
        brightness: 300.0,
    });

    // Point light
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            intensity: 2_000_000.0,
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(40.0, 50.0, 40.0),
        ..default()
    });

    // Secondary light for better visibility
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            intensity: 1_000_000.0,
            shadows_enabled: false,
            ..default()
        },
        transform: Transform::from_xyz(-40.0, 30.0, -40.0),
        ..default()
    });
}

/// System to update node label positions in screen space
fn update_node_labels(
    mut label_query: Query<(&NodeLabel, &mut Style, &mut Visibility)>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let Ok((camera, camera_transform)) = camera_query.get_single() else {
        return;
    };

    for (label, mut style, mut visibility) in label_query.iter_mut() {
        // Project 3D position to screen space
        let viewport_pos = camera.world_to_viewport(camera_transform, label.node_position);

        if let Some(screen_pos) = viewport_pos {
            // Position is visible, update the UI element position
            style.left = Val::Px(screen_pos.x);
            style.top = Val::Px(screen_pos.y - 20.0); // Offset above the node
            *visibility = Visibility::Visible;
        } else {
            // Position is behind camera, hide the label
            *visibility = Visibility::Hidden;
        }
    }
}
