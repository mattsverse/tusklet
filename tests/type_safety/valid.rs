use tusklet::model;
#[path = "../../src/store/entities/mod.rs"]
mod entities;

use entities::{database, project};
use sea_orm::{EntityTrait, QueryFilter, Set};

fn main() {
    let _ = project::Entity::find().filter(project::COLUMN.name.eq("Work"));
    let _ = database::ActiveModel {
        port: Set(5432),
        config: Set(model::DatabaseConfig::default()),
        ..Default::default()
    };
}
