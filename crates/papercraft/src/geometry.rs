//! The Geometry Interfaces for scripts outside a browser: `DOMPoint`, `DOMRect`, `DOMQuad` and
//! `DOMMatrix`, with their read-only versions, as browsers have them; the canvas takes and gives
//! them (`getTransform()`, `setTransform()`, `addPath()`). Their values are hidden properties of
//! each object (`_x`, `_m11` …) behind getters, and setters on the writable classes. As in a worker,
//! there's no CSS to parse: a matrix from a string throws a `TypeError`.

use crate::{Call, Engine, Error, HostFunction, Value, define_accessor, method, object};

/// The realm's `Object` and `Float32Array`, and the prototypes of the classes, by [`Kind`].
pub(crate) struct Classes<H> {
    object: H,
    float32: H,
    prototypes: Vec<H>,
}

/// The classes, in the order they're made.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    PointReadOnly,
    Point,
    RectReadOnly,
    Rect,
    Quad,
    MatrixReadOnly,
    Matrix,
}

const KINDS: [(Kind, &str); 7] = [
    (Kind::PointReadOnly, "DOMPointReadOnly"),
    (Kind::Point, "DOMPoint"),
    (Kind::RectReadOnly, "DOMRectReadOnly"),
    (Kind::Rect, "DOMRect"),
    (Kind::Quad, "DOMQuad"),
    (Kind::MatrixReadOnly, "DOMMatrixReadOnly"),
    (Kind::Matrix, "DOMMatrix"),
];

/// Every value an object keeps: its name, the key it's kept under, and for a matrix, its index in
/// the 16 values (`m11`, `m12`, `m13`, `m14`, `m21` …, as `toFloat64Array()` lists them).
const FIELDS: [(&str, &str, usize); 32] = [
    ("x", "_x", 0),
    ("y", "_y", 0),
    ("z", "_z", 0),
    ("w", "_w", 0),
    ("width", "_width", 0),
    ("height", "_height", 0),
    ("a", "_m11", 0),
    ("b", "_m12", 1),
    ("c", "_m21", 4),
    ("d", "_m22", 5),
    ("e", "_m41", 12),
    ("f", "_m42", 13),
    ("m11", "_m11", 0),
    ("m12", "_m12", 1),
    ("m13", "_m13", 2),
    ("m14", "_m14", 3),
    ("m21", "_m21", 4),
    ("m22", "_m22", 5),
    ("m23", "_m23", 6),
    ("m24", "_m24", 7),
    ("m31", "_m31", 8),
    ("m32", "_m32", 9),
    ("m33", "_m33", 10),
    ("m34", "_m34", 11),
    ("m41", "_m41", 12),
    ("m42", "_m42", 13),
    ("m43", "_m43", 14),
    ("m44", "_m44", 15),
    ("p1", "_p1", 0),
    ("p2", "_p2", 0),
    ("p3", "_p3", 0),
    ("p4", "_p4", 0),
];

/// The fields of the classes, by [`FIELDS`] index.
const POINT: [usize; 4] = [0, 1, 2, 3];
const RECT: [usize; 4] = [0, 1, 4, 5];
const MATRIX: std::ops::Range<usize> = 6..28;
const QUAD: std::ops::Range<usize> = 28..32;

/// The keys of a matrix's 16 values.
const KEYS: [&str; 16] =
    ["_m11", "_m12", "_m13", "_m14", "_m21", "_m22", "_m23", "_m24", "_m31", "_m32", "_m33", "_m34", "_m41", "_m42", "_m43", "_m44"];

/// The values only a 3D matrix has, with what they are in a 2D one.
const THREE_D: [(&str, usize, f64); 10] = [
    ("m13", 2, 0.0),
    ("m14", 3, 0.0),
    ("m23", 6, 0.0),
    ("m24", 7, 0.0),
    ("m31", 8, 0.0),
    ("m32", 9, 0.0),
    ("m33", 10, 1.0),
    ("m34", 11, 0.0),
    ("m43", 14, 0.0),
    ("m44", 15, 1.0),
];

const IDENTITY: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A matrix: its 16 values, column by column, and whether it's 2D.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix {
    pub m: [f64; 16],
    pub is_2d: bool,
}

impl Matrix {
    /// The 2D matrix `[a, b, c, d, e, f]`.
    pub(crate) fn new_2d([a, b, c, d, e, f]: [f64; 6]) -> Matrix {
        let mut m = IDENTITY;
        (m[0], m[1], m[4], m[5], m[12], m[13]) = (a, b, c, d, e, f);
        Matrix { m, is_2d: true }
    }

    /// `[a, b, c, d, e, f]`.
    pub(crate) fn coefficients(&self) -> [f64; 6] {
        let m = &self.m;
        [m[0], m[1], m[4], m[5], m[12], m[13]]
    }

    /// `self × other`: `other` applies first.
    fn multiply(&self, other: &Matrix) -> Matrix {
        let (a, b) = (&self.m, &other.m);
        let mut m = [0.0; 16];
        for (i, value) in m.iter_mut().enumerate() {
            let (column, row) = (i / 4, i % 4);
            *value = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
        Matrix { m, is_2d: self.is_2d && other.is_2d }
    }

    fn translation(x: f64, y: f64, z: f64) -> Matrix {
        let mut m = IDENTITY;
        (m[12], m[13], m[14]) = (x, y, z);
        Matrix { m, is_2d: z == 0.0 }
    }

    fn scaling(x: f64, y: f64, z: f64) -> Matrix {
        let mut m = IDENTITY;
        (m[0], m[5], m[10]) = (x, y, z);
        Matrix { m, is_2d: z == 1.0 }
    }

    /// A rotation of `degrees` around the vector `(x, y, z)`, as CSS's `rotate3d()`; 2D around z.
    fn rotation(x: f64, y: f64, z: f64, degrees: f64) -> Matrix {
        let is_2d = x == 0.0 && y == 0.0;
        let length = (x * x + y * y + z * z).sqrt();
        if length == 0.0 || !length.is_finite() {
            return Matrix { m: IDENTITY, is_2d };
        }
        let (x, y, z) = (x / length, y / length, z / length);
        let half = degrees.to_radians() / 2.0;
        let (sc, sq) = (half.sin() * half.cos(), half.sin().powi(2));
        #[rustfmt::skip]
        let m = [
            1.0 - 2.0 * (y * y + z * z) * sq, 2.0 * (x * y * sq + z * sc), 2.0 * (x * z * sq - y * sc), 0.0,
            2.0 * (x * y * sq - z * sc), 1.0 - 2.0 * (x * x + z * z) * sq, 2.0 * (y * z * sq + x * sc), 0.0,
            2.0 * (x * z * sq + y * sc), 2.0 * (y * z * sq - x * sc), 1.0 - 2.0 * (x * x + y * y) * sq, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        Matrix { m, is_2d }
    }

    /// The inverse; all NaN and 3D when there is none.
    fn inverse(&self) -> Matrix {
        let singular = Matrix { m: [f64::NAN; 16], is_2d: false };
        if self.m.iter().any(|v| !v.is_finite()) {
            return singular;
        }
        if self.is_2d {
            let [a, b, c, d, e, f] = self.coefficients();
            let det = a * d - b * c;
            if det == 0.0 {
                return singular;
            }
            return Matrix::new_2d([d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det]);
        }
        invert(&self.m).map_or(singular, |m| Matrix { m, is_2d: false })
    }

    /// `(x, y, z, w)` through the matrix.
    fn transform(&self, [x, y, z, w]: [f64; 4]) -> [f64; 4] {
        let m = &self.m;
        let row = |r: usize| m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r] * w;
        [row(0), row(1), row(2), row(3)]
    }
}

/// The inverse of a 4 × 4 matrix, by its cofactors.
#[rustfmt::skip]
fn invert(m: &[f64; 16]) -> Option<[f64; 16]> {
    let mut i = [0.0; 16];
    i[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    i[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    i[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    i[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    i[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    i[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    i[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    i[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    i[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    i[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    i[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    i[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    i[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    i[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    i[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    i[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    let det = m[0] * i[0] + m[1] * i[4] + m[2] * i[8] + m[3] * i[12];
    (det != 0.0 && det.is_finite()).then(|| i.map(|v| v / det))
}

// The matrix operations: the read-only class's give a new matrix (`translate()`), the writable
// class's change it (`translateSelf()`); an empty name is one the class doesn't have.

const OPERATIONS: [(&str, &str); 14] = [
    ("translate", "translateSelf"),
    ("scale", "scaleSelf"),
    ("scaleNonUniform", ""),
    ("scale3d", "scale3dSelf"),
    ("rotate", "rotateSelf"),
    ("rotateFromVector", "rotateFromVectorSelf"),
    ("rotateAxisAngle", "rotateAxisAngleSelf"),
    ("skewX", "skewXSelf"),
    ("skewY", "skewYSelf"),
    ("multiply", "multiplySelf"),
    ("", "preMultiplySelf"),
    ("flipX", ""),
    ("flipY", ""),
    ("inverse", "invertSelf"),
];

/// Operation `index` of [`OPERATIONS`] on `m`, with `args`.
fn operate<H: Clone>(call: &mut dyn Call<H>, index: usize, m: &Matrix, args: &[Value<H>]) -> Result<Matrix, Error> {
    let present = |i: usize| !matches!(args.get(i), None | Some(Value::Undefined));
    let mut n = |i: usize, default: f64| -> Result<f64, Error> {
        match args.get(i) {
            None | Some(Value::Undefined) => Ok(default),
            Some(value) => call.to_number(value),
        }
    };
    Ok(match index {
        0 => m.multiply(&Matrix::translation(n(0, 0.0)?, n(1, 0.0)?, n(2, 0.0)?)),
        1 | 3 => {
            let (sx, sy, sz, ox, oy, oz) = if index == 1 {
                let sx = n(0, 1.0)?;
                (sx, n(1, sx)?, n(2, 1.0)?, n(3, 0.0)?, n(4, 0.0)?, n(5, 0.0)?)
            } else {
                let s = n(0, 1.0)?;
                (s, s, s, n(1, 0.0)?, n(2, 0.0)?, n(3, 0.0)?)
            };
            m.multiply(&Matrix::translation(ox, oy, oz)).multiply(&Matrix::scaling(sx, sy, sz)).multiply(&Matrix::translation(-ox, -oy, -oz))
        }
        2 => m.multiply(&Matrix::scaling(n(0, 1.0)?, n(1, 1.0)?, 1.0)),
        4 => {
            // One angle is around z.
            let (x, y, z) = if !present(1) && !present(2) { (0.0, 0.0, n(0, 0.0)?) } else { (n(0, 0.0)?, n(1, 0.0)?, n(2, 0.0)?) };
            let mut result = m.multiply(&Matrix::rotation(0.0, 0.0, 1.0, z));
            if y != 0.0 {
                result = result.multiply(&Matrix::rotation(0.0, 1.0, 0.0, y));
            }
            if x != 0.0 {
                result = result.multiply(&Matrix::rotation(1.0, 0.0, 0.0, x));
            }
            result
        }
        5 => {
            let (x, y) = (n(0, 0.0)?, n(1, 0.0)?);
            let degrees = if x == 0.0 && y == 0.0 { 0.0 } else { y.atan2(x).to_degrees() };
            m.multiply(&Matrix::rotation(0.0, 0.0, 1.0, degrees))
        }
        6 => m.multiply(&Matrix::rotation(n(0, 0.0)?, n(1, 0.0)?, n(2, 0.0)?, n(3, 0.0)?)),
        7 => m.multiply(&Matrix::new_2d([1.0, 0.0, n(0, 0.0)?.to_radians().tan(), 1.0, 0.0, 0.0])),
        8 => m.multiply(&Matrix::new_2d([1.0, n(0, 0.0)?.to_radians().tan(), 0.0, 1.0, 0.0, 0.0])),
        9 => m.multiply(&matrix_init(call, args.first(), false)?),
        10 => matrix_init(call, args.first(), false)?.multiply(m),
        11 => m.multiply(&Matrix::new_2d([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        12 => m.multiply(&Matrix::new_2d([1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
        _ => m.inverse(),
    })
}

/// Operation `I` on the read-only class: a new `DOMMatrix`.
fn operation<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let m = read_matrix(call, this)?;
    let result = operate(call, I, &m, args)?;
    new_matrix(call, Kind::Matrix, &result).map(Value::Js)
}

/// Operation `I` on the writable class: `this`, changed.
fn operation_self<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let m = read_matrix(call, this)?;
    let result = operate(call, I, &m, args)?;
    let Value::Js(matrix) = this else {
        return Err(not(Kind::Matrix));
    };
    write_matrix(call, matrix, &result)?;
    Ok(this.clone())
}

// Installing.

/// The methods on the classes, and their static methods (`true`).
#[allow(clippy::type_complexity)]
fn functions<H: Clone>() -> Vec<(Kind, bool, &'static str, HostFunction<H>)> {
    use Kind::*;
    let mut functions: Vec<(Kind, bool, &'static str, HostFunction<H>)> = vec![
        (PointReadOnly, true, "fromPoint", |call, _, args| from_point(call, args, PointReadOnly)),
        (Point, true, "fromPoint", |call, _, args| from_point(call, args, Point)),
        (PointReadOnly, false, "matrixTransform", matrix_transform),
        (PointReadOnly, false, "toJSON", |call, this, _| to_json(call, this, &POINT.map(|i| FIELDS[i]))),
        (RectReadOnly, true, "fromRect", |call, _, args| from_rect(call, args, RectReadOnly)),
        (Rect, true, "fromRect", |call, _, args| from_rect(call, args, Rect)),
        (RectReadOnly, false, "toJSON", rect_json),
        (Quad, true, "fromRect", quad_from_rect),
        (Quad, true, "fromQuad", quad_from_quad),
        (Quad, false, "getBounds", quad_bounds),
        (Quad, false, "toJSON", |call, this, _| to_json(call, this, &FIELDS[QUAD])),
        (MatrixReadOnly, true, "fromMatrix", |call, _, args| from_matrix(call, args, MatrixReadOnly)),
        (Matrix, true, "fromMatrix", |call, _, args| from_matrix(call, args, Matrix)),
        (MatrixReadOnly, true, "fromFloat32Array", |call, _, args| from_array(call, args, MatrixReadOnly)),
        (Matrix, true, "fromFloat32Array", |call, _, args| from_array(call, args, Matrix)),
        (MatrixReadOnly, true, "fromFloat64Array", |call, _, args| from_array(call, args, MatrixReadOnly)),
        (Matrix, true, "fromFloat64Array", |call, _, args| from_array(call, args, Matrix)),
        (MatrixReadOnly, false, "transformPoint", transform_point),
        (MatrixReadOnly, false, "toFloat32Array", |call, this, _| to_array(call, this, true)),
        (MatrixReadOnly, false, "toFloat64Array", |call, this, _| to_array(call, this, false)),
        (MatrixReadOnly, false, "toJSON", matrix_json),
        (MatrixReadOnly, false, "toString", matrix_string),
        (Matrix, false, "setMatrixValue", |_, _, _| Err(Error::Type("setMatrixValue: there's no CSS to parse outside a window".into()))),
    ];
    macro_rules! operations {
        ($($i:literal)*) => {
            [$((operation::<H, $i> as HostFunction<H>, operation_self::<H, $i> as HostFunction<H>)),*]
        };
    }
    let functions_of: [(HostFunction<H>, HostFunction<H>); OPERATIONS.len()] = operations!(0 1 2 3 4 5 6 7 8 9 10 11 12 13);
    for ((name, self_name), (f, f_self)) in OPERATIONS.iter().zip(functions_of) {
        if !name.is_empty() {
            functions.push((MatrixReadOnly, false, name, f));
        }
        if !self_name.is_empty() {
            functions.push((Matrix, false, self_name, f_self));
        }
    }
    functions
}

/// The getters on the classes, and the setters on the writable ones.
#[allow(clippy::type_complexity)]
fn accessors<H: Clone>() -> Vec<(Kind, &'static str, HostFunction<H>, Option<HostFunction<H>>)> {
    use Kind::*;
    macro_rules! fields {
        ($($i:literal)*) => { [$((get::<H, $i> as HostFunction<H>, set::<H, $i> as HostFunction<H>)),*] };
    }
    let fields: [(HostFunction<H>, HostFunction<H>); FIELDS.len()] =
        fields!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31);
    let mut accessors = Vec::new();
    let mut add = |kind: Kind, indices: Vec<usize>, writable: bool| {
        for i in indices {
            if let (Some((name, ..)), Some((get, set))) = (FIELDS.get(i), fields.get(i)) {
                accessors.push((kind, *name, *get, writable.then_some(*set)));
            }
        }
    };
    add(PointReadOnly, POINT.to_vec(), false);
    add(Point, POINT.to_vec(), true);
    add(RectReadOnly, RECT.to_vec(), false);
    add(Rect, RECT.to_vec(), true);
    add(Quad, QUAD.collect(), false);
    add(MatrixReadOnly, MATRIX.collect(), false);
    add(Matrix, MATRIX.collect(), true);
    let sides: [(&str, HostFunction<H>); 4] = [("top", side::<H, 0>), ("right", side::<H, 1>), ("bottom", side::<H, 2>), ("left", side::<H, 3>)];
    for (name, get) in sides {
        accessors.push((RectReadOnly, name, get, None));
    }
    accessors.push((MatrixReadOnly, "is2D", |call, this, _| stored(call, this, "_is2D"), None));
    accessors.push((MatrixReadOnly, "isIdentity", |call, this, _| Ok(Value::Bool(read_matrix(call, this)?.m == IDENTITY)), None));
    accessors
}

/// Sets the classes on `global`, unless it has them (a browser's realm does).
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    if engine.with_call(|call| Ok(!matches!(call.get(global, "DOMMatrix")?, Value::Undefined)))? {
        return Ok(());
    }
    let constructors: [HostFunction<E::Handle>; 7] =
        [construct::<_, 0>, construct::<_, 1>, construct::<_, 2>, construct::<_, 3>, construct::<_, 4>, construct::<_, 5>, construct::<_, 6>];
    let mut made = Vec::new();
    for ((_, name), constructor) in KINDS.iter().zip(constructors) {
        made.push(engine.constructor(name, constructor)?);
    }
    let mut functions = Vec::new();
    for (kind, on_class, name, function) in self::functions::<E::Handle>() {
        functions.push((kind, on_class, name, engine.function(name, function)?));
    }
    let mut accessors = Vec::new();
    for (kind, name, get, set) in self::accessors::<E::Handle>() {
        let set = set.map(|set| engine.function(name, set)).transpose()?;
        accessors.push((kind, name, engine.function(name, get)?, set));
    }
    engine.with_call(|call| {
        let object_class = object(call, global, "Object")?;
        let mut prototypes: Vec<E::Handle> = Vec::new();
        for (i, ((kind, name), constructor)) in KINDS.iter().zip(&made).enumerate() {
            // A writable class inherits from its read-only one, as `DOMPoint` from
            // `DOMPointReadOnly`, its constructor too.
            let parent = matches!(kind, Kind::Point | Kind::Rect | Kind::Matrix).then(|| i.checked_sub(1)).flatten();
            let prototype = match parent.and_then(|p| prototypes.get(p)) {
                Some(parent) => match method(call, &object_class, "create", &[Value::Js(parent.clone())])? {
                    Value::Js(prototype) => prototype,
                    _ => return Err(Error::Script("Object.create made nothing".into())),
                },
                None => call.new_object()?,
            };
            if let Some(parent) = parent.and_then(|p| made.get(p)) {
                method(call, &object_class, "setPrototypeOf", &[Value::Js(constructor.clone()), Value::Js(parent.clone())])?;
            }
            call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
            call.set(constructor, "prototype", &Value::Js(prototype.clone()))?;
            call.set(global, name, &Value::Js(constructor.clone()))?;
            crate::tag(call, global, &prototype, name)?;
            prototypes.push(prototype);
        }
        for (kind, on_class, name, function) in functions {
            let target = if on_class { made.get(kind as usize) } else { prototypes.get(kind as usize) };
            if let Some(target) = target {
                call.set(target, name, &Value::Js(function))?;
            }
        }
        for (kind, name, get, set) in accessors {
            if let Some(prototype) = prototypes.get(kind as usize) {
                define_accessor(call, global, prototype, name, get, set)?;
            }
        }
        // The name WebKit gave `DOMMatrix`, which browsers keep.
        if let Some(matrix) = made.get(Kind::Matrix as usize) {
            call.set(global, "WebKitCSSMatrix", &Value::Js(matrix.clone()))?;
        }
        let classes = Classes { float32: object(call, global, "Float32Array")?, object: object_class, prototypes };
        call.host()?.borrow_mut().geometry = Some(classes);
        Ok(())
    })
}

// Objects.

fn not(kind: Kind) -> Error {
    let name = KINDS.iter().find(|(k, _)| *k == kind).map_or("", |(_, name)| name);
    Error::Type(format!("not a {name}"))
}

/// A new object of class `kind`.
fn create<H: Clone>(call: &mut dyn Call<H>, kind: Kind) -> Result<H, Error> {
    let (object_class, prototype) = {
        let host = call.host()?;
        let host = host.borrow();
        let classes = host.geometry.as_ref().ok_or_else(|| Error::Script("geometry is not set up".into()))?;
        (classes.object.clone(), classes.prototypes.get(kind as usize).cloned().ok_or_else(|| not(kind))?)
    };
    match method(call, &object_class, "create", &[Value::Js(prototype)])? {
        Value::Js(made) => Ok(made),
        _ => Err(Error::Script("Object.create made nothing".into())),
    }
}

/// What `this` keeps under `key`.
fn stored<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Value<H>, Error> {
    match this {
        Value::Js(object) => call.get(object, key),
        _ => Ok(Value::Undefined),
    }
}

fn number<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<f64, Error> {
    Ok(match stored(call, this, key)? {
        Value::Number(n) => n,
        _ => f64::NAN,
    })
}

/// The getter of `FIELDS[I]`.
fn get<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    match FIELDS.get(I) {
        Some((_, key, _)) => stored(call, this, key),
        None => Ok(Value::Undefined),
    }
}

/// The setter of `FIELDS[I]`: a matrix that gets a 3D value isn't 2D from then on.
fn set<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (Some((_, key, index)), Value::Js(object)) = (FIELDS.get(I), this) else {
        return Ok(Value::Undefined);
    };
    let value = call.to_number(args.first().unwrap_or(&Value::Undefined))?;
    call.set(object, key, &Value::Number(value))?;
    if MATRIX.contains(&I) && THREE_D.iter().any(|(_, i, default)| i == index && value != *default) {
        call.set(object, "_is2D", &Value::Bool(false))?;
    }
    Ok(Value::Undefined)
}

/// `NaN` when either is, as `Math.min()` and `Math.max()` have it.
fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// `rect.top`, `right`, `bottom` or `left` (`I`).
fn side<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let [x, y, w, h] = [number(call, this, "_x")?, number(call, this, "_y")?, number(call, this, "_width")?, number(call, this, "_height")?];
    Ok(Value::Number(match I {
        0 => min(y, y + h),
        1 => max(x, x + w),
        2 => max(y, y + h),
        _ => min(x, x + w),
    }))
}

/// The `n`th argument as a number, `default` when it's missing.
fn argument<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], n: usize, default: f64) -> Result<f64, Error> {
    match args.get(n) {
        None | Some(Value::Undefined) => Ok(default),
        Some(value) => call.to_number(value),
    }
}

/// `dictionary[key]` as a number, when it's there.
fn member<H: Clone>(call: &mut dyn Call<H>, dictionary: Option<&H>, key: &str) -> Result<Option<f64>, Error> {
    let Some(dictionary) = dictionary else {
        return Ok(None);
    };
    match call.get(dictionary, key)? {
        Value::Undefined => Ok(None),
        value => call.to_number(&value).map(Some),
    }
}

/// The object of a dictionary argument: none for `undefined` and `null`.
fn dictionary<H: Clone>(value: Option<&Value<H>>, what: &str) -> Result<Option<H>, Error> {
    match value {
        None | Some(Value::Undefined) | Some(Value::Null) => Ok(None),
        Some(Value::Js(object)) => Ok(Some(object.clone())),
        Some(_) => Err(Error::Type(format!("not a {what}"))),
    }
}

fn new_point<H: Clone>(call: &mut dyn Call<H>, kind: Kind, values: [f64; 4]) -> Result<H, Error> {
    let point = create(call, kind)?;
    for (i, value) in POINT.into_iter().zip(values) {
        if let Some((_, key, _)) = FIELDS.get(i) {
            call.set(&point, key, &Value::Number(value))?;
        }
    }
    Ok(point)
}

fn new_rect<H: Clone>(call: &mut dyn Call<H>, kind: Kind, values: [f64; 4]) -> Result<H, Error> {
    let rect = create(call, kind)?;
    for (i, value) in RECT.into_iter().zip(values) {
        if let Some((_, key, _)) = FIELDS.get(i) {
            call.set(&rect, key, &Value::Number(value))?;
        }
    }
    Ok(rect)
}

/// A `DOMPointInit`'s `x`, `y`, `z` and `w`.
fn point_init<H: Clone>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<[f64; 4], Error> {
    let init = dictionary(value, "DOMPointInit")?;
    Ok([
        member(call, init.as_ref(), "x")?.unwrap_or(0.0),
        member(call, init.as_ref(), "y")?.unwrap_or(0.0),
        member(call, init.as_ref(), "z")?.unwrap_or(0.0),
        member(call, init.as_ref(), "w")?.unwrap_or(1.0),
    ])
}

/// A `DOMRectInit`'s `x`, `y`, `width` and `height`.
fn rect_init<H: Clone>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<[f64; 4], Error> {
    let init = dictionary(value, "DOMRectInit")?;
    let mut values = [0.0; 4];
    for (value, key) in values.iter_mut().zip(["x", "y", "width", "height"]) {
        *value = member(call, init.as_ref(), key)?.unwrap_or(0.0);
    }
    Ok(values)
}

fn read_point<H: Clone>(call: &mut dyn Call<H>, point: &Value<H>) -> Result<[f64; 4], Error> {
    Ok([number(call, point, "_x")?, number(call, point, "_y")?, number(call, point, "_z")?, number(call, point, "_w")?])
}

/// The matrix `this` is.
fn read_matrix<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Matrix, Error> {
    let mut m = [0.0; 16];
    for (value, key) in m.iter_mut().zip(KEYS) {
        *value = number(call, this, key)?;
    }
    Ok(Matrix { m, is_2d: matches!(stored(call, this, "_is2D")?, Value::Bool(true)) })
}

fn write_matrix<H: Clone>(call: &mut dyn Call<H>, object: &H, matrix: &Matrix) -> Result<(), Error> {
    for (key, value) in KEYS.iter().zip(matrix.m) {
        call.set(object, key, &Value::Number(value))?;
    }
    call.set(object, "_is2D", &Value::Bool(matrix.is_2d))
}

fn new_matrix<H: Clone>(call: &mut dyn Call<H>, kind: Kind, matrix: &Matrix) -> Result<H, Error> {
    let made = create(call, kind)?;
    write_matrix(call, &made, matrix)?;
    Ok(made)
}

/// The numbers of the geometry object `value`, whose class `Object.prototype.toString()` names
/// (`[object DOMPoint]` …), as structured serialization keeps them: a point's four, a rect's four,
/// a quad's points' sixteen, a matrix's sixteen and whether it's 2D. None for another object.
pub(crate) fn serialize<H: Clone>(call: &mut dyn Call<H>, value: &Value<H>, class: &str) -> Result<Option<Vec<f64>>, Error> {
    let Some((kind, _)) = KINDS.iter().find(|(_, name)| class == format!("[object {name}]")) else {
        return Ok(None);
    };
    Ok(Some(match kind {
        Kind::PointReadOnly | Kind::Point => read_point(call, value)?.to_vec(),
        Kind::RectReadOnly | Kind::Rect => {
            let mut values = Vec::with_capacity(4);
            for i in RECT {
                values.push(number(call, value, FIELDS[i].1)?);
            }
            values
        }
        Kind::Quad => {
            let mut values = Vec::with_capacity(16);
            for (_, key, _) in &FIELDS[QUAD] {
                let point = stored(call, value, key)?;
                values.extend(read_point(call, &point)?);
            }
            values
        }
        Kind::MatrixReadOnly | Kind::Matrix => {
            let matrix = read_matrix(call, value)?;
            let mut values = matrix.m.to_vec();
            values.push(if matrix.is_2d { 1.0 } else { 0.0 });
            values
        }
    }))
}

/// The geometry object of class `class` (as [`serialize`] names it) with `values`; none for
/// another class or numbers that don't fit it.
pub(crate) fn deserialize<H: Clone>(call: &mut dyn Call<H>, class: &str, values: &[f64]) -> Result<Option<Value<H>>, Error> {
    let Some((kind, _)) = KINDS.iter().find(|(_, name)| class == format!("[object {name}]")) else {
        return Ok(None);
    };
    let four = |values: &[f64]| <[f64; 4]>::try_from(values).ok();
    let made = match kind {
        Kind::PointReadOnly | Kind::Point => match four(values) {
            Some(values) => new_point(call, *kind, values)?,
            None => return Ok(None),
        },
        Kind::RectReadOnly | Kind::Rect => match four(values) {
            Some(values) => new_rect(call, *kind, values)?,
            None => return Ok(None),
        },
        Kind::Quad => {
            let points: Vec<[f64; 4]> = values.chunks(4).filter_map(four).collect();
            match <[[f64; 4]; 4]>::try_from(points) {
                Ok(points) => new_quad(call, points)?,
                Err(_) => return Ok(None),
            }
        }
        Kind::MatrixReadOnly | Kind::Matrix => match values.split_last().and_then(|(is_2d, m)| Some((*is_2d, <[f64; 16]>::try_from(m).ok()?))) {
            Some((is_2d, m)) => new_matrix(call, *kind, &Matrix { m, is_2d: is_2d == 1.0 })?,
            None => return Ok(None),
        },
    };
    Ok(Some(Value::Js(made)))
}

/// A new `DOMMatrix` of `matrix`, as `getTransform()` gives it.
pub(crate) fn matrix_value<H: Clone>(call: &mut dyn Call<H>, matrix: &Matrix) -> Result<Value<H>, Error> {
    new_matrix(call, Kind::Matrix, matrix).map(Value::Js)
}

/// The matrix of a `DOMMatrixInit` (or a `DOMMatrix2DInit`, `two_d`), checked and completed as the
/// standard says: `a` and `m11` must agree, a 2D one has no 3D values.
pub(crate) fn matrix_init<H: Clone>(call: &mut dyn Call<H>, value: Option<&Value<H>>, two_d: bool) -> Result<Matrix, Error> {
    let init = dictionary(value, "DOMMatrixInit")?;
    let init = init.as_ref();
    let mut m = IDENTITY;
    for (alias, name, index) in [("a", "m11", 0), ("b", "m12", 1), ("c", "m21", 4), ("d", "m22", 5), ("e", "m41", 12), ("f", "m42", 13)] {
        let (alias, value) = (member(call, init, alias)?, member(call, init, name)?);
        if let (Some(a), Some(v)) = (alias, value)
            && !(a == v || a.is_nan() && v.is_nan())
        {
            return Err(Error::Type(format!("the matrix's {name} and its alias disagree")));
        }
        if let (Some(value), Some(slot)) = (value.or(alias), m.get_mut(index)) {
            *slot = value;
        }
    }
    if two_d {
        return Ok(Matrix { m, is_2d: true });
    }
    let is_2d = match init {
        Some(init) => match call.get(init, "is2D")? {
            Value::Undefined => None,
            value => Some(crate::truthy(&value)),
        },
        None => None,
    };
    let mut three_d = false;
    for (name, index, default) in THREE_D {
        if let Some(value) = member(call, init, name)? {
            if let Some(slot) = m.get_mut(index) {
                *slot = value;
            }
            three_d |= value != default;
        }
    }
    if is_2d == Some(true) && three_d {
        return Err(Error::Type("a 2D matrix has 3D values".into()));
    }
    Ok(Matrix { m, is_2d: is_2d.unwrap_or(!three_d) })
}

// Constructors and static methods.

/// The constructor of `KINDS[K]`.
fn construct<H: Clone, const K: usize>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some((kind, _)) = KINDS.get(K) else {
        return Err(Error::Type("Illegal constructor".into()));
    };
    let kind = *kind;
    match kind {
        Kind::PointReadOnly | Kind::Point => {
            let values = [argument(call, args, 0, 0.0)?, argument(call, args, 1, 0.0)?, argument(call, args, 2, 0.0)?, argument(call, args, 3, 1.0)?];
            new_point(call, kind, values).map(Value::Js)
        }
        Kind::RectReadOnly | Kind::Rect => {
            let values = [argument(call, args, 0, 0.0)?, argument(call, args, 1, 0.0)?, argument(call, args, 2, 0.0)?, argument(call, args, 3, 0.0)?];
            new_rect(call, kind, values).map(Value::Js)
        }
        Kind::Quad => {
            let points =
                [point_init(call, args.first())?, point_init(call, args.get(1))?, point_init(call, args.get(2))?, point_init(call, args.get(3))?];
            new_quad(call, points).map(Value::Js)
        }
        Kind::MatrixReadOnly | Kind::Matrix => {
            let matrix = match args.first() {
                None | Some(Value::Undefined) => Matrix { m: IDENTITY, is_2d: true },
                Some(Value::String(_)) => return Err(Error::Type("there's no CSS to parse a matrix from outside a window".into())),
                Some(Value::Js(list)) => matrix_from_list(call, list)?,
                Some(_) => return Err(Error::Type("not a matrix".into())),
            };
            new_matrix(call, kind, &matrix).map(Value::Js)
        }
    }
}

/// The matrix of a list of 6 (2D) or 16 numbers.
fn matrix_from_list<H: Clone>(call: &mut dyn Call<H>, list: &H) -> Result<Matrix, Error> {
    let length = match call.get(list, "length")? {
        Value::Number(n) => n,
        _ => return Err(Error::Type("a matrix is 6 or 16 numbers".into())),
    };
    let mut values = Vec::with_capacity(16);
    if length == 6.0 || length == 16.0 {
        for i in 0..length as usize {
            let value = call.get(list, &i.to_string())?;
            values.push(call.to_number(&value)?);
        }
    }
    match values.as_slice() {
        [a, b, c, d, e, f] => Ok(Matrix::new_2d([*a, *b, *c, *d, *e, *f])),
        values => match <[f64; 16]>::try_from(values) {
            Ok(m) => Ok(Matrix { m, is_2d: false }),
            Err(_) => Err(Error::Type("a matrix is 6 or 16 numbers".into())),
        },
    }
}

fn from_point<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], kind: Kind) -> Result<Value<H>, Error> {
    let values = point_init(call, args.first())?;
    new_point(call, kind, values).map(Value::Js)
}

fn from_rect<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], kind: Kind) -> Result<Value<H>, Error> {
    let values = rect_init(call, args.first())?;
    new_rect(call, kind, values).map(Value::Js)
}

fn from_matrix<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], kind: Kind) -> Result<Value<H>, Error> {
    let matrix = matrix_init(call, args.first(), false)?;
    new_matrix(call, kind, &matrix).map(Value::Js)
}

fn from_array<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], kind: Kind) -> Result<Value<H>, Error> {
    let Some(Value::Js(list)) = args.first() else {
        return Err(Error::Type("not a typed array".into()));
    };
    let matrix = matrix_from_list(call, list)?;
    new_matrix(call, kind, &matrix).map(Value::Js)
}

fn new_quad<H: Clone>(call: &mut dyn Call<H>, points: [[f64; 4]; 4]) -> Result<H, Error> {
    let quad = create(call, Kind::Quad)?;
    for ((_, key, _), values) in FIELDS[QUAD].iter().zip(points) {
        let point = new_point(call, Kind::Point, values)?;
        call.set(&quad, key, &Value::Js(point))?;
    }
    Ok(quad)
}

fn quad_from_rect<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let [x, y, w, h] = rect_init(call, args.first())?;
    new_quad(call, [[x, y, 0.0, 1.0], [x + w, y, 0.0, 1.0], [x + w, y + h, 0.0, 1.0], [x, y + h, 0.0, 1.0]]).map(Value::Js)
}

fn quad_from_quad<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let init = dictionary(args.first(), "DOMQuadInit")?;
    let mut points = [[0.0, 0.0, 0.0, 1.0]; 4];
    for (point, name) in points.iter_mut().zip(["p1", "p2", "p3", "p4"]) {
        if let Some(init) = &init {
            let value = call.get(init, name)?;
            *point = point_init(call, Some(&value))?;
        }
    }
    new_quad(call, points).map(Value::Js)
}

/// `quad.getBounds()`: the `DOMRect` around its points.
fn quad_bounds<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut points = Vec::with_capacity(4);
    for (_, key, _) in &FIELDS[QUAD] {
        let point = stored(call, this, key)?;
        points.push(read_point(call, &point)?);
    }
    let (xs, ys) = (points.iter().map(|p| p[0]), points.iter().map(|p| p[1]));
    let (left, right) = (xs.clone().fold(f64::INFINITY, min), xs.fold(f64::NEG_INFINITY, max));
    let (top, bottom) = (ys.clone().fold(f64::INFINITY, min), ys.fold(f64::NEG_INFINITY, max));
    new_rect(call, Kind::Rect, [left, top, right - left, bottom - top]).map(Value::Js)
}

// Methods.

/// `point.matrixTransform(matrix)`: a new `DOMPoint`.
fn matrix_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let matrix = matrix_init(call, args.first(), false)?;
    let point = read_point(call, this)?;
    new_point(call, Kind::Point, matrix.transform(point)).map(Value::Js)
}

/// `matrix.transformPoint(point)`: a new `DOMPoint`.
fn transform_point<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let matrix = read_matrix(call, this)?;
    let point = point_init(call, args.first())?;
    new_point(call, Kind::Point, matrix.transform(point)).map(Value::Js)
}

/// `toFloat32Array()` or `toFloat64Array()`.
fn to_array<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, float32: bool) -> Result<Value<H>, Error> {
    let matrix = read_matrix(call, this)?;
    let array = call.new_float64_array(&matrix.m)?;
    if !float32 {
        return Ok(Value::Js(array));
    }
    let class = call.host()?.borrow().geometry.as_ref().map(|c| c.float32.clone()).ok_or_else(|| Error::Script("geometry is not set up".into()))?;
    call.construct(&class, &[Value::Js(array)])
}

/// An object of the values `fields` name, as `toJSON()` gives them.
fn to_json<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, fields: &[(&str, &str, usize)]) -> Result<Value<H>, Error> {
    let json = call.new_object()?;
    for (name, key, _) in fields {
        let value = stored(call, this, key)?;
        call.set(&json, name, &value)?;
    }
    Ok(Value::Js(json))
}

fn rect_json<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let json = to_json(call, this, &RECT.map(|i| FIELDS[i]))?;
    if let Value::Js(object) = &json {
        let sides: [(&str, HostFunction<H>); 4] = [("top", side::<H, 0>), ("right", side::<H, 1>), ("bottom", side::<H, 2>), ("left", side::<H, 3>)];
        for (name, side) in sides {
            let value = side(call, this, args)?;
            call.set(object, name, &value)?;
        }
    }
    Ok(json)
}

fn matrix_json<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let json = to_json(call, this, &FIELDS[MATRIX])?;
    let matrix = read_matrix(call, this)?;
    if let Value::Js(object) = &json {
        call.set(object, "is2D", &Value::Bool(matrix.is_2d))?;
        call.set(object, "isIdentity", &Value::Bool(matrix.m == IDENTITY))?;
    }
    Ok(json)
}

/// `String(matrix)`: `matrix(a, b, c, d, e, f)` or `matrix3d(…)`, its numbers as JavaScript
/// writes them; an `InvalidStateError` for one that isn't finite.
fn matrix_string<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let matrix = read_matrix(call, this)?;
    if matrix.m.iter().any(|v| !v.is_finite()) {
        return Err(Error::Dom("InvalidStateError", "the matrix isn't finite".into()));
    }
    let values: Vec<f64> = if matrix.is_2d { matrix.coefficients().to_vec() } else { matrix.m.to_vec() };
    let mut parts = Vec::with_capacity(values.len());
    for value in values {
        parts.push(call.to_string(&Value::Number(value))?);
    }
    let name = if matrix.is_2d { "matrix" } else { "matrix3d" };
    Ok(Value::String(format!("{name}({})", parts.join(", "))))
}
