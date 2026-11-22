use super::*;

fn block() -> MapBlock {
    MapBlock {
        map_x: 0,
        map_y: 0,
        map_z: 10,
        tiles: vec![0; 256],
        hidden: vec![false; 256],
        water: vec![0; 256],
        magma: vec![0; 256],
        ..Default::default()
    }
}

#[test]
fn should_hide_real_tile_types_when_tiles_are_unseen() {
    let mut block = block();
    block.hidden[0] = true;
    block.water[0] = 7;
    block.magma[0] = 3;
    let value = floor_plan(
        &BlockList {
            map_blocks: vec![block],
            ..Default::default()
        },
        &[Tiletype {
            id: 0,
            shape: Some(TiletypeShape::Wall as i32),
            ..Default::default()
        }],
        Bounds {
            x1: 0,
            y1: 0,
            x2: 0,
            y2: 0,
        },
        10,
    )
    .unwrap();

    assert_eq!(value["rows"][0], "?");
}

#[test]
fn should_classify_visible_liquids_before_tile_shape() {
    let mut block = block();
    block.water[0] = 7;
    block.magma[1] = 3;
    let value = floor_plan(
        &BlockList {
            map_blocks: vec![block],
            ..Default::default()
        },
        &[Tiletype {
            id: 0,
            shape: Some(TiletypeShape::Wall as i32),
            ..Default::default()
        }],
        Bounds {
            x1: 0,
            y1: 0,
            x2: 1,
            y2: 0,
        },
        10,
    )
    .unwrap();

    assert_eq!(value["rows"][0], "~M");
}

#[test]
fn should_preserve_requested_shape_when_clamped_at_map_edge() {
    let bounds = Bounds::from_center(1000, 1, 10, 4).clamp(192, 192);

    assert_eq!(
        bounds,
        Bounds {
            x1: 182,
            y1: 0,
            x2: 191,
            y2: 3
        }
    );
}

#[test]
fn should_reject_regions_larger_than_output_limits() {
    assert!(validate_plan_size(MAX_PLAN_WIDTH + 1, 1).is_err());
    assert!(validate_plan_size(1, MAX_PLAN_HEIGHT + 1).is_err());
}
