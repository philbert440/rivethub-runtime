use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::plugin::action::ViewAction;
use crate::plugin::view::{ViewAlign, ViewContext};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasScene {
    pub items: Vec<CanvasItem>,
    pub grid: Option<CanvasGrid>,
    pub viewport: Option<CanvasViewport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasGrid {
    pub size: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub snap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasViewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CanvasItem {
    #[serde(rename_all = "camelCase")]
    Node {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        shapes: Vec<CanvasShape>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        anchors: Vec<CanvasAnchor>,
        tooltip: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        menu: Vec<CanvasMenuItem>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        locked: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        decorative: bool,
    },
    #[serde(rename_all = "camelCase")]
    Connector {
        id: String,
        from: CanvasEnd,
        to: CanvasEnd,
        route: CanvasRoute,
        stroke: Option<ColorRole>,
        stroke_width: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dashed: bool,
        arrow: CanvasArrow,
        label: Option<String>,
        tooltip: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        menu: Vec<CanvasMenuItem>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        decorative: bool,
    },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CanvasShape {
    #[serde(rename_all = "camelCase")]
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        radius: Option<f64>,
        fill: Option<ColorRole>,
        stroke: Option<ColorRole>,
        stroke_width: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Ellipse {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        fill: Option<ColorRole>,
        stroke: Option<ColorRole>,
        stroke_width: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        x: f64,
        y: f64,
        text: String,
        size: Option<f64>,
        weight: Option<CanvasWeight>,
        color: Option<ColorRole>,
        align: Option<ViewAlign>,
        max_width: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        monospace: bool,
    },
    #[serde(rename_all = "camelCase")]
    Path {
        d: String,
        fill: Option<ColorRole>,
        stroke: Option<ColorRole>,
        stroke_width: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dashed: bool,
    },
    #[serde(rename_all = "camelCase")]
    Icon {
        x: f64,
        y: f64,
        name: String,
        size: Option<f64>,
        color: Option<ColorRole>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasAnchor {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub side: CanvasSide,
    pub direction: CanvasDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CanvasSide {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CanvasDirection {
    In,
    Out,
    Both,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasEnd {
    pub node: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CanvasRoute {
    Straight,
    Curved,
    Orthogonal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CanvasArrow {
    None,
    End,
    Start,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CanvasWeight {
    Regular,
    Medium,
    Bold,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ColorRole {
    Surface,
    SurfaceContainer,
    SurfaceContainerHigh,
    Primary,
    PrimaryContainer,
    OnPrimary,
    OnPrimaryContainer,
    SecondaryContainer,
    OnSurface,
    OnSurfaceVariant,
    Outline,
    OutlineVariant,
    Info,
    Success,
    Warning,
    Danger,
    Category1,
    Category2,
    Category3,
    Category4,
    Category5,
    Category6,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasMenuItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub danger: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasEventRequest {
    pub event: CanvasEvent,
    pub params: Option<JsonObject>,
    #[serde(default)]
    pub context: ViewContext,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CanvasEvent {
    #[serde(rename_all = "camelCase")]
    Selected { items: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Moved { items: Vec<CanvasPosition> },
    #[serde(rename_all = "camelCase")]
    Connected { from: CanvasEnd, to: CanvasEnd },
    #[serde(rename_all = "camelCase")]
    Deleted { items: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Activated { item: String },
    #[serde(rename_all = "camelCase")]
    Menu {
        item: Option<String>,
        choice: String,
        x: f64,
        y: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasPosition {
    pub id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CanvasResponse {
    #[serde(default)]
    pub ops: Vec<CanvasOp>,
    #[serde(default)]
    pub actions: Vec<ViewAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CanvasOp {
    #[serde(rename_all = "camelCase")]
    Upsert { item: CanvasItem },
    #[serde(rename_all = "camelCase")]
    Remove { ids: Vec<String> },
}
