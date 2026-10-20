//! Times loading Paper.js and running scripts, in every engine built in:
//! `cargo run --release -p vectorcraft-scriptographer --example measure`.

use std::time::{Duration, Instant};

use vectorcraft_scriptographer::{Engine, Error, Runtime};

const CIRCLE: &str = "new Path.Circle({ center: view.bounds.center, radius: 100, fillColor: 'red' })";

/// A heavy script: a path that grows by 100 points, smoothed after each, with offsets computed by
/// PaperScript operators.
const DRAG: &str = "var path = new Path({ strokeColor: 'black' })\n\
    for (var i = 0; i < 100; i++) {\n\
        var point = new Point(i * 5, 100) + new Point({ length: 20, angle: i * 10 })\n\
        path.add(point)\n\
        path.smooth()\n\
    }";

/// One drag event on a path that already has 100 points: add a point, smooth once.
const DRAG_STEP: &str = "var path = project.activeLayer.lastChild\n\
    path.add(path.lastSegment.point + [5, 0])\n\
    path.smooth()";

fn time<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<(T, Duration), Error> {
    let start = Instant::now();
    let result = f()?;
    Ok((result, start.elapsed()))
}

fn measure<E: Engine>(name: &str) -> Result<(), Error> {
    println!("{name}:");
    let (mut runtime, load) = time(Runtime::<E>::new)?;
    println!("  create engine + load Paper.js:      {load:?}");
    // V8 starts later runtimes from a snapshot it made with the first. The app runs each script
    // on a thread of its own.
    let (_, load) = time(Runtime::<E>::new)?;
    println!("  another engine:                     {load:?}");
    let thread = std::thread::spawn(|| time(Runtime::<E>::new).map(|(_, load)| load));
    let load = thread.join().map_err(|_| Error::Script("the thread failed".into()))??;
    println!("  another engine, on a new thread:    {load:?}");

    let (scope, setup) = time(|| runtime.create_scope(800.0, 600.0))?;
    println!("  first PaperScope + setup:           {setup:?}");
    let (_, setup) = time(|| runtime.create_scope(800.0, 600.0))?;
    println!("  second PaperScope + setup:          {setup:?}");

    let (_, circle) = time(|| runtime.execute(&scope, CIRCLE))?;
    println!("  compile + run circle script, first: {circle:?}");
    let (_, circle) = time(|| runtime.execute(&scope, CIRCLE))?;
    println!("  compile + run circle script, again: {circle:?}");

    let (_, drag) = time(|| runtime.execute(&scope, DRAG))?;
    println!("  100 × add + smooth, first:          {drag:?}");
    let (_, drag) = time(|| runtime.execute(&scope, DRAG))?;
    println!("  100 × add + smooth, again:          {drag:?}");

    let mut total = Duration::ZERO;
    for _ in 0..100 {
        total += time(|| runtime.execute(&scope, DRAG_STEP))?.1;
    }
    println!("  one drag step (add + smooth), avg:  {:?}", total / 100);
    Ok(())
}

fn main() -> Result<(), Error> {
    #[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
    measure::<vectorcraft_scriptographer::V8>("V8")?;
    #[cfg(feature = "boa")]
    measure::<vectorcraft_scriptographer::Boa>("Boa")?;
    Ok(())
}
