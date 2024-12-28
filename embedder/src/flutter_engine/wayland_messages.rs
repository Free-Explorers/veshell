use serde::ser::SerializeStruct;
use serde::Serialize;
use smithay::output::{Mode, Output, PhysicalProperties};
use smithay::utils::{Buffer as BufferCoords, Logical, Point, Rectangle, Size};
use std::collections::HashMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceMessage {
    pub surface_id: u64,
    pub role: Option<SurfaceRole>,
    pub texture_id: i64,
    pub buffer_delta: Option<MyPoint<i32, Logical>>,
    pub buffer_size: Option<MySize<i32, BufferCoords>>,
    pub scale: i32,
    pub input_region: MyRectangle<i32, Logical>,
    pub subsurfaces_below: Vec<u64>,
    pub subsurfaces_above: Vec<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum SurfaceRole {
    XdgSurface(XdgSurfaceMessage),
    Subsurface(SubsurfaceMessage),
    X11Surface,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XdgSurfaceMessage {
    pub geometry: Option<MyRectangle<i32, Logical>>,
    pub role: Option<XdgSurfaceRole>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsurfaceMessage {
    pub position: MyPoint<i32, Logical>,
    pub parent: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum XdgSurfaceRole {
    XdgToplevel(ToplevelMessage),
    XdgPopup(PopupMessage),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToplevelMessage {
    pub parent_surface_id: Option<u64>,
    pub app_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupMessage {
    pub parent: u64,
    pub position: MyPoint<i32, Logical>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorsMessage {
    pub monitors: Vec<MyOutput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewX11Surface {
    pub x11_surface_id: u64,
    pub override_redirect: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapX11Surface {
    pub x11_surface_id: u64,
    pub geometry: MyRectangle<i32, Logical>,
    pub parent: Option<u64>,
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MyPoint<N, Kind>(pub Point<N, Kind>);

impl<N, Kind> From<Point<N, Kind>> for MyPoint<N, Kind> {
    fn from(point: Point<N, Kind>) -> Self {
        MyPoint(point)
    }
}

impl<N, Kind> Serialize for MyPoint<N, Kind>
where
    N: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("Point", 2)?;
        state.serialize_field("x", &self.0.x)?;
        state.serialize_field("y", &self.0.y)?;
        state.end()
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MySize<N, Kind>(pub Size<N, Kind>);

impl<N, Kind> From<Size<N, Kind>> for MySize<N, Kind> {
    fn from(size: Size<N, Kind>) -> Self {
        MySize(size)
    }
}

impl<N, Kind> Serialize for MySize<N, Kind>
where
    N: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("Size", 2)?;
        state.serialize_field("width", &self.0.w)?;
        state.serialize_field("height", &self.0.h)?;
        state.end()
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MyRectangle<N, Kind>(pub Rectangle<N, Kind>);

impl<N, Kind> From<Rectangle<N, Kind>> for MyRectangle<N, Kind> {
    fn from(rectangle: Rectangle<N, Kind>) -> Self {
        MyRectangle(rectangle)
    }
}

impl<N, Kind> Serialize for MyRectangle<N, Kind>
where
    N: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("Rectangle", 4)?;
        state.serialize_field("x", &self.0.loc.x)?;
        state.serialize_field("y", &self.0.loc.y)?;
        state.serialize_field("width", &self.0.size.w)?;
        state.serialize_field("height", &self.0.size.h)?;
        state.end()
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MyOutput(pub Output);

impl From<Output> for MyOutput {
    fn from(output: Output) -> Self {
        MyOutput(output)
    }
}

impl Serialize for MyOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let output = &self.0;
        let mut state = serializer.serialize_struct("Output", 7)?;
        state.serialize_field("name", &output.name())?;
        state.serialize_field("description", &output.description())?;
        state.serialize_field(
            "physicalProperties",
            &MyPhysicalProperties(output.physical_properties()),
        )?;
        state.serialize_field("scale", &output.current_scale().fractional_scale())?;
        state.serialize_field("location", &MyPoint(output.current_location()))?;
        state.serialize_field("currentMode", &output.current_mode().map(MyMode))?;
        state.serialize_field("preferredMode", &output.preferred_mode().map(MyMode))?;
        state.serialize_field(
            "modes",
            &output.modes().into_iter().map(MyMode).collect::<Vec<_>>(),
        )?;
        state.end()
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct MyMode(pub Mode);

impl From<Mode> for MyMode {
    fn from(mode: Mode) -> Self {
        MyMode(mode)
    }
}

impl Serialize for MyMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mode = &self.0;
        let mut state = serializer.serialize_struct("Mode", 2)?;
        state.serialize_field("size", &MySize(mode.size))?;
        state.serialize_field("refreshRate", &mode.refresh)?;
        state.end()
    }
}

#[repr(transparent)]
#[derive(Debug)]
struct MyPhysicalProperties(PhysicalProperties);

impl From<PhysicalProperties> for MyPhysicalProperties {
    fn from(physical_properties: PhysicalProperties) -> Self {
        MyPhysicalProperties(physical_properties)
    }
}

impl Serialize for MyPhysicalProperties {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let properties = &self.0;
        let mut state = serializer.serialize_struct("PhysicalProperties", 3)?;
        state.serialize_field("size", &MySize(properties.size))?;
        state.serialize_field("make", &properties.make)?;
        state.serialize_field("model", &properties.model)?;
        state.end()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentVariables<'k, 'v> {
    pub environment_variables: HashMap<&'k str, Option<&'v str>>,
}
