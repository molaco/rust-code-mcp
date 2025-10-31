//! UI systems for hypergraph visualization

use crate::hypergraph::viz::{EdgeMarker, NodeTypeMarker, TypeVisibility, LegendUI};
use bevy::prelude::*;

/// Setup the legend UI showing node types and controls
pub fn setup_legend(mut commands: Commands) {
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    position_type: PositionType::Absolute,
                    left: Val::Px(20.0),
                    top: Val::Px(20.0),
                    min_width: Val::Px(300.0),
                    min_height: Val::Px(400.0),
                    padding: UiRect::all(Val::Px(15.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    ..default()
                },
                background_color: Color::srgba(0.1, 0.1, 0.1, 0.9).into(),
                ..default()
            },
            LegendUI,
        ))
        .with_children(|parent| {
            // Title
            parent.spawn(TextBundle::from_section(
                "Node Types (Toggle with keys)",
                TextStyle {
                    font_size: 20.0,
                    color: Color::WHITE,
                    ..default()
                },
            ));

            // Function
            parent.spawn(TextBundle::from_section(
                "F - 🟢 Functions",
                TextStyle {
                    font_size: 16.0,
                    color: Color::srgb(0.0, 1.0, 0.3),
                    ..default()
                },
            ));

            // Struct
            parent.spawn(TextBundle::from_section(
                "S - 🔵 Structs",
                TextStyle {
                    font_size: 16.0,
                    color: Color::srgb(0.2, 0.5, 1.0),
                    ..default()
                },
            ));

            // Trait
            parent.spawn(TextBundle::from_section(
                "T - 🟣 Traits",
                TextStyle {
                    font_size: 16.0,
                    color: Color::srgb(1.0, 0.0, 1.0),
                    ..default()
                },
            ));

            // Enum
            parent.spawn(TextBundle::from_section(
                "E - 🟡 Enums",
                TextStyle {
                    font_size: 16.0,
                    color: Color::srgb(1.0, 0.9, 0.0),
                    ..default()
                },
            ));

            // Other
            parent.spawn(TextBundle::from_section(
                "O - ⚪ Other",
                TextStyle {
                    font_size: 16.0,
                    color: Color::srgb(0.9, 0.9, 0.9),
                    ..default()
                },
            ));

            // Separator
            parent.spawn(TextBundle::from_section(
                "---",
                TextStyle {
                    font_size: 14.0,
                    color: Color::srgb(0.5, 0.5, 0.5),
                    ..default()
                },
            ));

            // Show All
            parent.spawn(TextBundle::from_section(
                "A - Show All",
                TextStyle {
                    font_size: 16.0,
                    color: Color::WHITE,
                    ..default()
                },
            ));
        });
}

/// Handle keyboard input to toggle visibility
pub fn handle_keyboard_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visibility: ResMut<TypeVisibility>,
) {
    if keyboard.just_pressed(KeyCode::KeyF) {
        visibility.function = !visibility.function;
        println!("Functions: {}", if visibility.function { "ON" } else { "OFF" });
    }
    if keyboard.just_pressed(KeyCode::KeyS) {
        visibility.struct_type = !visibility.struct_type;
        println!("Structs: {}", if visibility.struct_type { "ON" } else { "OFF" });
    }
    if keyboard.just_pressed(KeyCode::KeyT) {
        visibility.trait_type = !visibility.trait_type;
        println!("Traits: {}", if visibility.trait_type { "ON" } else { "OFF" });
    }
    if keyboard.just_pressed(KeyCode::KeyE) {
        visibility.enum_type = !visibility.enum_type;
        println!("Enums: {}", if visibility.enum_type { "ON" } else { "OFF" });
    }
    if keyboard.just_pressed(KeyCode::KeyO) {
        visibility.other = !visibility.other;
        println!("Other: {}", if visibility.other { "ON" } else { "OFF" });
    }
    if keyboard.just_pressed(KeyCode::KeyA) {
        visibility.function = true;
        visibility.struct_type = true;
        visibility.trait_type = true;
        visibility.enum_type = true;
        visibility.other = true;
        println!("All types: ON");
    }
}

/// Update node visibility based on current state
pub fn update_node_visibility(
    visibility: Res<TypeVisibility>,
    mut node_query: Query<(&NodeTypeMarker, &mut Visibility), Without<EdgeMarker>>,
    mut edge_query: Query<(&EdgeMarker, &mut Visibility)>,
) {
    if !visibility.is_changed() {
        return;
    }

    // Update node and text label visibility (both have NodeTypeMarker)
    for (marker, mut vis) in node_query.iter_mut() {
        *vis = match marker {
            NodeTypeMarker::Function => {
                if visibility.function {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                }
            }
            NodeTypeMarker::Struct => {
                if visibility.struct_type {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                }
            }
            NodeTypeMarker::Trait => {
                if visibility.trait_type {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                }
            }
            NodeTypeMarker::Enum => {
                if visibility.enum_type {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                }
            }
            NodeTypeMarker::Other => {
                if visibility.other {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                }
            }
        };
    }

    // Update edge visibility - hide if ANY connected node type is hidden
    for (edge_marker, mut vis) in edge_query.iter_mut() {
        let mut should_show = true;

        // Check if any source type is hidden
        for source_type in &edge_marker.source_types {
            if !is_type_visible(&visibility, source_type) {
                should_show = false;
                break;
            }
        }

        // Check if any target type is hidden
        if should_show {
            for target_type in &edge_marker.target_types {
                if !is_type_visible(&visibility, target_type) {
                    should_show = false;
                    break;
                }
            }
        }

        *vis = if should_show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Helper to check if a node type is visible
fn is_type_visible(visibility: &TypeVisibility, marker: &NodeTypeMarker) -> bool {
    match marker {
        NodeTypeMarker::Function => visibility.function,
        NodeTypeMarker::Struct => visibility.struct_type,
        NodeTypeMarker::Trait => visibility.trait_type,
        NodeTypeMarker::Enum => visibility.enum_type,
        NodeTypeMarker::Other => visibility.other,
    }
}
