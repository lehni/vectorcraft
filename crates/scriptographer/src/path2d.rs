//! `Path2D` for scripts outside a browser: a path to fill, stroke, clip with or test points
//! against, built with the context's path methods (in its own coordinates; the context's transform
//! applies when it's drawn) or from SVG path data. Its path is kept in Rust, as a canvas's pixels
//! are, and freed when the script drops it.

use svgtypes::{SimplePathSegment, SimplifyingPathParser};
use vello_cpu::kurbo::{Affine, BezPath};

use crate::canvas::store;
use crate::{Call, Error, Value, method};

/// `new Path2D()`, `new Path2D(path)` (a copy) or `new Path2D(svgPathData)`.
pub(crate) fn construct<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let path = match args.first() {
        Some(Value::String(data)) => parse(data),
        Some(other) => path_of(call, other)?.unwrap_or_default(),
        None => BezPath::new(),
    };
    let (object_class, prototype, registry) = {
        let host = call.host()?;
        let host = host.borrow();
        let canvases = host.canvases.as_ref().ok_or_else(|| Error::Script("Path2D is not set up".into()))?;
        (canvases.object.clone(), canvases.path_prototype.clone(), canvases.path_registry.clone())
    };
    let Value::Js(object) = method(call, &object_class, "create", &[Value::Js(prototype)])? else {
        return Err(Error::Script("Object.create made nothing".into()));
    };
    let number = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        let canvases = host.canvases.as_mut().ok_or_else(|| Error::Script("Path2D is not set up".into()))?;
        store(&mut canvases.paths, path)
    };
    call.set(&object, "_path2d", &Value::Number(number as f64))?;
    if let Some(registry) = registry {
        method(call, &registry, "register", &[Value::Js(object.clone()), Value::Number(number as f64)])?;
    }
    Ok(Value::Js(object))
}

/// Frees the path numbered `args[0]`, which the script dropped.
pub(crate) fn release<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Number(number)) = args.first()
        && let Some(canvases) = call.host()?.borrow_mut().canvases.as_mut()
        && let Some(slot) = canvases.paths.get_mut(*number as usize)
    {
        *slot = None;
    }
    Ok(Value::Undefined)
}

/// The number of the `Path2D` `value` is, if it's one.
fn number<H>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<Option<usize>, Error> {
    let Value::Js(object) = value else {
        return Ok(None);
    };
    Ok(match call.get(object, "_path2d")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => Some(n as usize),
        _ => None,
    })
}

/// `f` with the path of the `Path2D` `this` is.
pub(crate) fn with_path<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut BezPath) -> R) -> Result<R, Error> {
    let not_a_path = || Error::Type("not a Path2D".into());
    let number = number(call, this)?.ok_or_else(not_a_path)?;
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let path = host.canvases.as_mut().and_then(|c| c.paths.get_mut(number)).and_then(Option::as_mut).ok_or_else(not_a_path)?;
    Ok(f(path))
}

/// The path of `value`, when it's a `Path2D`.
pub(crate) fn path_of<H>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<Option<BezPath>, Error> {
    let Some(number) = number(call, value)? else {
        return Ok(None);
    };
    let host = call.host()?;
    let host = host.borrow();
    Ok(host.canvases.as_ref().and_then(|c| c.paths.get(number)).and_then(Option::as_ref).cloned())
}

/// `addPath(path, transform)`: adds `path`'s subpaths, through `transform` (a `DOMMatrix2DInit`)
/// when there is one.
pub(crate) fn add_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(added) = args.first().map(|a| path_of(call, a)).transpose()?.flatten() else {
        return Err(Error::Type("addPath: not a Path2D".into()));
    };
    let coefficients = crate::geometry::matrix_init(call, args.get(1), true)?.coefficients();
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
