//! `Path2D` for scripts outside a browser: a path to fill, stroke, clip with or test points
//! against, built with the context's path methods (in its own coordinates; the context's transform
//! applies when it's drawn) or from SVG path data. Its path is kept in a slot, as a canvas's pixels
//! are, which `js/src/canvas.js` frees when the script drops it.

use svgtypes::{SimplePathSegment, SimplifyingPathParser};
use vello_cpu::kurbo::{Affine, BezPath};

use crate::canvas::store;
use crate::{Call, Error, Value};

/// `createPath(source)`: the slot of a new path, empty, a copy of the path in the slot `source`,
/// or of SVG path data.
pub(crate) fn construct<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let path = match args.first() {
        Some(Value::String(data)) => parse(data),
        Some(source) => path_of(call, source)?.unwrap_or_default(),
        None => BezPath::new(),
    };
    crate::canvas::with_canvases(call, |canvases| Value::Number(store(&mut canvases.paths, path) as f64))
}

/// `f` with the path of the `Path2D` `this` is.
pub(crate) fn with_path<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut BezPath) -> R) -> Result<R, Error> {
    let slot = crate::canvas::slot(call, this)?;
    crate::canvas::with_canvases(call, |canvases| canvases.paths.get_mut(slot).and_then(Option::as_mut).map(f))?
        .ok_or_else(|| Error::Type("not a Path2D".into()))
}

/// The path in the slot `value` names, when it's one.
pub(crate) fn path_of<H>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<Option<BezPath>, Error> {
    let Value::Number(slot) = value else {
        return Ok(None);
    };
    let slot = *slot as usize;
    crate::canvas::with_canvases(call, |canvases| canvases.paths.get(slot).and_then(Option::as_ref).cloned())
}

/// `addPath(path, transform)`: adds `path`'s subpaths, through `transform` (a `DOMMatrix2DInit`)
/// when there is one.
pub(crate) fn add_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(added) = args.first().map(|a| path_of(call, a)).transpose()?.flatten() else {
        return Err(Error::Type("addPath: not a Path2D".into()));
    };
    let coefficients = crate::drawing::matrix_coefficients(call, args.get(1))?;
    // A matrix that isn't finite adds nothing, as in browsers.
    if coefficients.iter().any(|c| !c.is_finite()) {
        return Ok(Value::Undefined);
    }
    let transform = Affine::new(coefficients);
    with_path(call, this, |path| path.extend(transform * added))?;
    Ok(Value::Undefined)
}

/// The path SVG path data describes, up to its first error, as browsers read it.
fn parse(data: &str) -> BezPath {
    let mut path = BezPath::new();
    for segment in SimplifyingPathParser::from(data) {
        match segment {
            Ok(SimplePathSegment::MoveTo { x, y }) => path.move_to((x, y)),
            Ok(SimplePathSegment::LineTo { x, y }) => path.line_to((x, y)),
            Ok(SimplePathSegment::CurveTo { x1, y1, x2, y2, x, y }) => path.curve_to((x1, y1), (x2, y2), (x, y)),
            Ok(SimplePathSegment::Quadratic { x1, y1, x, y }) => path.quad_to((x1, y1), (x, y)),
            Ok(SimplePathSegment::ClosePath) => path.close_path(),
            Err(_) => break,
        }
    }
    path
}
