// SeaORM generates async relation hooks even for models without loaded relations.
#![allow(clippy::unused_async_trait_impl)]

use sea_orm::entity::prelude::*;

#[sea_orm::compact_model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "projects")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::database::Entity")]
    Databases,
}

impl Related<super::database::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Databases.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
