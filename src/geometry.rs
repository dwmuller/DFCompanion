use dfhack_remote::{BlockList, MapBlock, Tiletype, TiletypeShape};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const DEFAULT_PLAN_WIDTH: i32 = 48;
pub const DEFAULT_PLAN_HEIGHT: i32 = 48;
pub const MAX_PLAN_WIDTH: i32 = 96;
pub const MAX_PLAN_HEIGHT: i32 = 64;

const BLOCK_WIDTH: i32 = 16;
const TILES_PER_BLOCK: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

impl Bounds {
    pub fn from_center(center_x: i32, center_y: i32, width: i32, height: i32) -> Self {
        Self {
            x1: center_x - width / 2,
            y1: center_y - height / 2,
            x2: center_x + (width - 1) / 2,
            y2: center_y + (height - 1) / 2,
        }
    }

    pub fn width(self) -> i32 {
        self.x2 - self.x1 + 1
    }

    pub fn height(self) -> i32 {
        self.y2 - self.y1 + 1
    }

    pub fn clamp(self, map_width: i32, map_height: i32) -> Self {
        let width = self.width().min(map_width);
        let height = self.height().min(map_height);
        let x1 = self.x1.clamp(0, map_width - width);
        let y1 = self.y1.clamp(0, map_height - height);
        Self {
            x1,
            y1,
            x2: x1 + width - 1,
            y2: y1 + height - 1,
        }
    }
}

pub fn block_bounds(bounds: Bounds) -> (i32, i32, i32, i32) {
    (
        bounds.x1.div_euclid(BLOCK_WIDTH),
        bounds.y1.div_euclid(BLOCK_WIDTH),
        (bounds.x2 + BLOCK_WIDTH) / BLOCK_WIDTH,
        (bounds.y2 + BLOCK_WIDTH) / BLOCK_WIDTH,
    )
}

pub fn validate_plan_size(width: i32, height: i32) -> Result<(), String> {
    if !(1..=MAX_PLAN_WIDTH).contains(&width) {
        return Err(format!("width must be between 1 and {MAX_PLAN_WIDTH}."));
    }
    if !(1..=MAX_PLAN_HEIGHT).contains(&height) {
        return Err(format!("height must be between 1 and {MAX_PLAN_HEIGHT}."));
    }
    Ok(())
}

/// Shapes RemoteFortressReader blocks into a player-visible floor plan. Hidden
/// tiles are replaced before tiletype classification so their true shape,
/// liquids, and construction state cannot escape into an AI tool reply.
pub fn floor_plan(
    blocks: &BlockList,
    tiletypes: &[Tiletype],
    bounds: Bounds,
    z: i64,
) -> Result<Value, String> {
    let tiletype_shapes = tiletypes
        .iter()
        .filter_map(|tiletype| {
            tiletype
                .shape
                .and_then(|shape| TiletypeShape::try_from(shape).ok())
                .map(|shape| (tiletype.id, shape))
        })
        .collect::<BTreeMap<_, _>>();
    let blocks = blocks
        .map_blocks
        .iter()
        .map(|block| ((block.map_x, block.map_y), block))
        .collect::<BTreeMap<_, _>>();
    let mut counts = BTreeMap::new();
    let mut rows = Vec::with_capacity(bounds.height() as usize);

    for y in bounds.y1..=bounds.y2 {
        let mut row = String::with_capacity(bounds.width() as usize);
        for x in bounds.x1..=bounds.x2 {
            let block_x = x.div_euclid(BLOCK_WIDTH) * BLOCK_WIDTH;
            let block_y = y.div_euclid(BLOCK_WIDTH) * BLOCK_WIDTH;
            let index =
                (y.rem_euclid(BLOCK_WIDTH) * BLOCK_WIDTH + x.rem_euclid(BLOCK_WIDTH)) as usize;
            let symbol = blocks
                .get(&(block_x, block_y))
                .map(|block| classify_tile(block, &tiletype_shapes, index))
                .unwrap_or('?');
            *counts.entry(symbol).or_insert(0_usize) += 1;
            row.push(symbol);
        }
        rows.push(row);
    }

    Ok(json!({
        "z": z,
        "bounds": {
            "x1": bounds.x1,
            "y1": bounds.y1,
            "x2": bounds.x2,
            "y2": bounds.y2
        },
        "coordinate_order": "Rows run from y1 to y2; characters within each row run from x1 to x2.",
        "rows": rows,
        "legend": {
            "?": "hidden or unavailable",
            " ": "open space",
            ".": "floor",
            "#": "wall or fortification",
            "^": "up stair",
            "v": "down stair",
            "X": "up/down stair",
            "/": "ramp",
            "\"": "ramp top",
            "o": "boulder or pebbles",
            "T": "tree or vegetation",
            "~": "water",
            "M": "magma",
        },
        "symbol_counts": counts
            .into_iter()
            .map(|(symbol, count)| (symbol.to_string(), json!(count)))
            .collect::<serde_json::Map<_, _>>(),
    }))
}

fn classify_tile(block: &MapBlock, tiletypes: &BTreeMap<i32, TiletypeShape>, index: usize) -> char {
    if index >= TILES_PER_BLOCK
        || block.hidden.get(index).copied().unwrap_or(true)
        || block.tiles.len() < TILES_PER_BLOCK
    {
        return '?';
    }
    if block.magma.get(index).copied().unwrap_or_default() > 0 {
        return 'M';
    }
    if block.water.get(index).copied().unwrap_or_default() > 0 {
        return '~';
    }
    match tiletypes
        .get(block.tiles.get(index).unwrap())
        .copied()
        .unwrap_or(TiletypeShape::NoShape)
    {
        TiletypeShape::Empty | TiletypeShape::BrookTop => ' ',
        TiletypeShape::Floor | TiletypeShape::BrookBed => '.',
        TiletypeShape::Wall | TiletypeShape::Fortification => '#',
        TiletypeShape::StairUp => '^',
        TiletypeShape::StairDown => 'v',
        TiletypeShape::StairUpdown => 'X',
        TiletypeShape::Ramp => '/',
        TiletypeShape::RampTop => '"',
        TiletypeShape::Boulder | TiletypeShape::Pebbles => 'o',
        TiletypeShape::TreeShape
        | TiletypeShape::Sapling
        | TiletypeShape::Shrub
        | TiletypeShape::Branch
        | TiletypeShape::TrunkBranch
        | TiletypeShape::Twig => 'T',
        TiletypeShape::EndlessPit | TiletypeShape::NoShape => '?',
    }
}

#[cfg(test)]
mod tests;
