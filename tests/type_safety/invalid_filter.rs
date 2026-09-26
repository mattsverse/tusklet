use tusklet::model;
#[path = "../../src/store/entities/mod.rs"]
mod entities;

use entities::project;
use sea_orm::{EntityTrait, QueryFilter};

fn main() {
    let _ = project::Entity::find().filter(project::COLUMN.name.eq(123));
}
