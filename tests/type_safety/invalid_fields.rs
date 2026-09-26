use tusklet::model;
#[path = "../../src/store/entities/mod.rs"]
mod entities;

use entities::database;
use sea_orm::Set;

fn main() {
    let _ = database::ActiveModel {
        port: Set("5432".to_owned()),
        config: Set("{}".to_owned()),
        ..Default::default()
    };
}
