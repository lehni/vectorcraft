//! `console` as browsers have it, printing to the script's output ([`Runtime::take_output`]
//! (crate::Runtime::take_output)): `log`, `info`, `warn`, `error`, `debug`, `trace` and `dir`, with
//! `%s`, `%d`, `%i`, `%f`, `%o`, `%O` and `%c` in the first argument; `assert`, `count`,
//! `countReset`, `time`, `timeLog`, `timeEnd`, `group`, `groupCollapsed`, `groupEnd` (what
//! follows a group is indented) and `table`; and Scriptographer's `print()`, which logs. Values
//! read as a browser's console shows them: strings at the top as they are, arrays as `[1, 2]`,
//! plain objects as `{a: 1}`, functions as `ƒ name()`, errors with their stack, and anything with
//! a `toString()` of its own (Paper.js's `Point`) as it writes itself.

use crate::{Call, Engine, Error, HostFunction, Value, method, object};

/// How deep arrays and objects are written, and how many of their items.
const DEPTH: usize = 2;
const ITEMS: usize = 100;

/// The console's state: its group depth, counters and timers, and what it uses of the realm.
pub(crate) struct Console<H> {
    indent: usize,
    counts: Vec<(String, u64)>,
    timers: Vec<(String, f64)>,
    /// `Object.prototype.toString`, `Object.keys`'s `Object`, and `performance`, when the realm
    /// has it.
    to_string: H,
    object: H,
    performance: Option<H>,
}

/// Sets `console` and `print` on `global`.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let methods: [(&str, HostFunction<E::Handle>); 19] = [
        ("log", log),
        ("info", log),
        ("warn", log),
        ("error", log),
        ("debug", log),
        ("dir", dir),
        ("dirxml", log),
        ("trace", trace),
        ("assert", assert),
        ("count", count),
        ("countReset", count_reset),
        ("time", time),
        ("timeLog", |call, this, args| time_log(call, this, args, false)),
        ("timeEnd", |call, this, args| time_log(call, this, args, true)),
        ("group", group),
        ("groupCollapsed", group),
        ("groupEnd", group_end),
        ("table", table),
        ("clear", |_, _, _| Ok(Value::Undefined)),
    ];
    let mut made = Vec::new();
    for (name, f) in methods {
        made.push((name, engine.function(name, f)?));
    }
    engine.with_call(|call| {
        let console = call.new_object()?;
        for (name, function) in &made {
            call.set(&console, name, &Value::Js(function.clone()))?;
        }
        call.set(global, "console", &Value::Js(console))?;
        if let Some((_, log)) = made.first() {
            call.set(global, "print", &Value::Js(log.clone()))?;
        }
        let object_class = object(call, global, "Object")?;
        let prototype = object(call, &object_class, "prototype")?;
        let state = Console {
            indent: 0,
            counts: Vec::new(),
            timers: Vec::new(),
            to_string: object(call, &prototype, "toString")?,
            object: object_class,
            performance: match call.get(global, "performance")? {
                Value::Js(performance) => Some(performance),
                _ => None,
            },
        };
        call.host()?.borrow_mut().console = Some(state);
        Ok(())
    })
}

/// Prints `line`, each of its lines indented as deep as the groups go.
fn print<H: Clone>(call: &mut dyn Call<H>, line: &str) -> Result<(), Error> {
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let indent = "  ".repeat(host.console.as_ref().map_or(0, |c| c.indent));
    let line = line.split('\n').map(|l| format!("{indent}{l}")).collect::<Vec<_>>().join("\n");
    host.output.push(line);
    Ok(())
}

/// The realm's handles the console uses.
fn shared<H: Clone>(call: &mut dyn Call<H>) -> Result<(H, H, Option<H>), Error> {
    let host = call.host()?;
    let host = host.borrow();
    let console = host.console.as_ref().ok_or_else(|| Error::Script("console is not set up".into()))?;
    Ok((console.to_string.clone(), console.object.clone(), console.performance.clone()))
}

/// `args` as one line: the first one's `%` directives filled in with those after it, the rest
/// after it, as consoles write them.
fn format<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<String, Error> {
    let mut parts = Vec::with_capacity(args.len());
    let mut rest = args;
    if let Some((Value::String(first), after)) = args.split_first()
        && first.contains('%')
    {
        let mut line = String::new();
        let mut after = after.iter();
        let mut chars = first.chars().peekable();
        while let Some(c) = chars.next() {
            let directive = match (c, chars.peek()) {
                ('%', Some(d)) if "sdifoOc%".contains(*d) => *d,
                _ => {
                    line.push(c);
                    continue;
                }
            };
            chars.next();
            if directive == '%' {
                line.push('%');
                continue;
            }
            let Some(value) = after.next() else {
                line.push('%');
                line.push(directive);
                continue;
            };
            match directive {
                's' => line.push_str(&inspect(call, value, 0)?),
                'd' | 'i' => {
                    let n = call.to_number(value)?;
                    line.push_str(&call.to_string(&Value::Number(if n.is_finite() { n.trunc() } else { n }))?);
                }
                'f' => {
                    let n = call.to_number(value)?;
                    line.push_str(&call.to_string(&Value::Number(n))?);
                }
                'o' | 'O' => line.push_str(&inspect(call, value, 1)?),
                // CSS for the line, which the output can't show.
                _ => {}
            }
        }
        parts.push(line);
        rest = after.as_slice();
    }
    for value in rest {
        parts.push(inspect(call, value, 0)?);
    }
    Ok(parts.join(" "))
}

/// `value` as a console writes it, `depth` deep in arrays and objects (where strings are quoted).
fn inspect<H: Clone>(call: &mut dyn Call<H>, value: &Value<H>, depth: usize) -> Result<String, Error> {
    let object = match value {
        Value::String(text) if depth == 0 => return Ok(text.clone()),
        Value::String(text) => return Ok(format!("'{text}'")),
        Value::Js(object) => object,
        other => return call.to_string(other),
    };
    let (to_string, object_class, _) = shared(call)?;
    let tag = call.call(&to_string, value, &[])?;
    let tag = call.to_string(&tag)?;
    match tag.as_str() {
        "[object Array]" => {
            if depth > DEPTH {
                return Ok("[Array]".into());
            }
            let count = length(call, object)?;
            let mut items = Vec::with_capacity(count.min(ITEMS));
            for i in 0..count.min(ITEMS) {
                let item = call.get(object, &i.to_string())?;
                items.push(inspect(call, &item, depth + 1)?);
            }
            if count > ITEMS {
                items.push(format!("… {} more", count - ITEMS));
            }
            Ok(format!("[{}]", items.join(", ")))
        }
        "[object Function]" | "[object AsyncFunction]" | "[object GeneratorFunction]" => {
            let name = call.get(object, "name")?;
            Ok(format!("ƒ {}()", call.to_string(&name)?))
        }
        "[object Error]" => match call.get(object, "stack")? {
            Value::String(stack) if !stack.is_empty() => Ok(stack),
            _ => call.to_string(value),
        },
        _ => {
            // An object that writes itself (`toString()` of its own) is written so.
            let written = call.to_string(value)?;
            if !written.starts_with("[object ") {
                return Ok(written);
            }
            if depth > DEPTH {
                return Ok("{…}".into());
            }
            let keys = method(call, &object_class, "keys", std::slice::from_ref(value))?;
            let Value::Js(keys) = keys else {
                return Ok(written);
            };
            let count = length(call, &keys)?;
            let mut members = Vec::with_capacity(count.min(ITEMS));
            for i in 0..count.min(ITEMS) {
                let key = call.get(&keys, &i.to_string())?;
                let key = call.to_string(&key)?;
                let member = call.get(object, &key)?;
                members.push(format!("{key}: {}", inspect(call, &member, depth + 1)?));
            }
            if count > ITEMS {
                members.push(format!("… {} more", count - ITEMS));
            }
            // Named for its class, unless it's a plain object.
            let class = match call.get(object, "constructor")? {
                Value::Js(constructor) => match call.get(&constructor, "name")? {
                    Value::String(name) if name != "Object" && !name.is_empty() => format!("{name} "),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            Ok(format!("{class}{{{}}}", members.join(", ")))
        }
    }
}

fn length<H: Clone>(call: &mut dyn Call<H>, object: &H) -> Result<usize, Error> {
    Ok(match call.get(object, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    })
}

/// The `label` argument, `default` without one.
fn label<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<String, Error> {
    match args.first() {
        None | Some(Value::Undefined) => Ok("default".into()),
        Some(label) => call.to_string(label),
    }
}

/// The console's clock, in milliseconds: `performance.now()`, else the runtime's frame time.
fn now<H: Clone>(call: &mut dyn Call<H>) -> Result<f64, Error> {
    let (_, _, performance) = shared(call)?;
    match performance {
        Some(performance) => {
            let now = method(call, &performance, "now", &[])?;
            call.to_number(&now)
        }
        None => Ok(call.host()?.borrow().now),
    }
}

fn with_console<H, R>(call: &mut dyn Call<H>, f: impl FnOnce(&mut Console<H>) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let console = host.console.as_mut().ok_or_else(|| Error::Script("console is not set up".into()))?;
    Ok(f(console))
}

fn log<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let line = format(call, args)?;
    print(call, &line)?;
    Ok(Value::Undefined)
}

/// `console.dir(value)`: the value as an object, strings quoted.
fn dir<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let line = inspect(call, args.first().unwrap_or(&Value::Undefined), 1)?;
    print(call, &line)?;
    Ok(Value::Undefined)
}

/// `console.trace(...)`: the line, then where it was called from.
fn trace<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let line = format(call, args)?;
    let mut text = if line.is_empty() { "Trace".to_owned() } else { format!("Trace: {line}") };
    if let Some(stack) = stack(call)? {
        text.push('\n');
        text.push_str(&stack);
    }
    print(call, &text)?;
    Ok(Value::Undefined)
}

/// The script's stack where it called the console, without the error's own line.
fn stack<H: Clone>(call: &mut dyn Call<H>) -> Result<Option<String>, Error> {
    let error = crate::exception::value(call, &Error::Script(String::new()))?;
    let Value::Js(error) = error else {
        return Ok(None);
    };
    Ok(match call.get(&error, "stack")? {
        Value::String(stack) => Some(stack.lines().skip(1).map(str::trim).collect::<Vec<_>>().join("\n")).filter(|s| !s.is_empty()),
        _ => None,
    })
}

fn assert<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if crate::truthy(args.first().unwrap_or(&Value::Undefined)) {
        return Ok(Value::Undefined);
    }
    let rest = args.get(1..).unwrap_or_default();
    let line = if rest.is_empty() { "Assertion failed".to_owned() } else { format!("Assertion failed: {}", format(call, rest)?) };
    print(call, &line)?;
    Ok(Value::Undefined)
}

fn count<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let label = label(call, args)?;
    let n = with_console(call, |c| match c.counts.iter_mut().find(|(l, _)| *l == label) {
        Some((_, n)) => {
            *n += 1;
            *n
        }
        None => {
            c.counts.push((label.clone(), 1));
            1
        }
    })?;
    print(call, &format!("{label}: {n}"))?;
    Ok(Value::Undefined)
}

fn count_reset<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let label = label(call, args)?;
    if !with_console(call, |c| c.counts.iter_mut().find(|(l, _)| *l == label).map(|(_, n)| *n = 0).is_some())? {
        print(call, &format!("Count for '{label}' does not exist"))?;
    }
    Ok(Value::Undefined)
}

fn time<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let label = label(call, args)?;
    let now = now(call)?;
    if !with_console(call, |c| {
        let exists = c.timers.iter().any(|(l, _)| *l == label);
        if !exists {
            c.timers.push((label.clone(), now));
        }
        !exists
    })? {
        print(call, &format!("Timer '{label}' already exists"))?;
    }
    Ok(Value::Undefined)
}

/// `timeLog(label, ...data)`, or `timeEnd(label)` (`end`), which stops the timer.
fn time_log<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>], end: bool) -> Result<Value<H>, Error> {
    let label = label(call, args)?;
    let now = now(call)?;
    let started = with_console(call, |c| {
        let at = c.timers.iter().position(|(l, _)| *l == label)?;
        let started = c.timers.get(at).map(|(_, t)| *t);
        if end {
            c.timers.remove(at);
        }
        started
    })?;
    let Some(started) = started else {
        print(call, &format!("Timer '{label}' does not exist"))?;
        return Ok(Value::Undefined);
    };
    let mut line = format!("{label}: {:.3} ms", now - started);
    let data = if end { &[][..] } else { args.get(1..).unwrap_or_default() };
    if !data.is_empty() {
        line.push(' ');
        line.push_str(&format(call, data)?);
    }
    print(call, &line)?;
    Ok(Value::Undefined)
}

fn group<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if !args.is_empty() {
        let line = format(call, args)?;
        print(call, &line)?;
    }
    with_console(call, |c| c.indent += 1)?;
    Ok(Value::Undefined)
}

fn group_end<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    with_console(call, |c| c.indent = c.indent.saturating_sub(1))?;
    Ok(Value::Undefined)
}

/// A row of `console.table()`: its key, its cells by column, or its value when it has no columns.
struct Row {
    key: String,
    cells: Vec<(String, String)>,
    value: Option<String>,
}

/// `console.table(data[, columns])`: an array's or object's items as rows, their properties as
/// columns, in text.
fn table<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(data)) = args.first() else {
        return log(call, this, args);
    };
    let (_, object_class, _) = shared(call)?;
    let Value::Js(keys) = method(call, &object_class, "keys", &[Value::Js(data.clone())])? else {
        return log(call, this, args);
    };
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    for i in 0..length(call, &keys)?.min(ITEMS) {
        let key = call.get(&keys, &i.to_string())?;
        let key = call.to_string(&key)?;
        let row = call.get(data, &key)?;
        let mut cells = Vec::new();
        let mut value = None;
        match &row {
            Value::Js(row_object) => {
                if let Value::Js(row_keys) = method(call, &object_class, "keys", std::slice::from_ref(&row))? {
                    for j in 0..length(call, &row_keys)?.min(ITEMS) {
                        let column = call.get(&row_keys, &j.to_string())?;
                        let column = call.to_string(&column)?;
                        let cell = call.get(row_object, &column)?;
                        cells.push((column.clone(), inspect(call, &cell, 1)?));
                        if !columns.contains(&column) {
                            columns.push(column);
                        }
                    }
                }
            }
            other => value = Some(inspect(call, other, 1)?),
        }
        rows.push(Row { key, cells, value });
    }
    // Only the columns asked for, when they are.
    if let Some(Value::Js(chosen)) = args.get(1) {
        let mut only = Vec::new();
        for i in 0..length(call, chosen)? {
            let column = call.get(chosen, &i.to_string())?;
            only.push(call.to_string(&column)?);
        }
        columns = only;
    }
    let has_values = rows.iter().any(|row| row.value.is_some());
    let mut header = vec!["(index)".to_owned()];
    header.extend(columns.iter().cloned());
    if has_values {
        header.push("Value".into());
    }
    let mut grid = vec![header];
    for Row { key, cells, value } in rows {
        let mut line = vec![key];
        for column in &columns {
            line.push(cells.iter().find(|(c, _)| c == column).map(|(_, v)| v.clone()).unwrap_or_default());
        }
        if has_values {
            line.push(value.unwrap_or_default());
        }
        grid.push(line);
    }
    let widths: Vec<usize> =
        (0..grid.first().map_or(0, Vec::len)).map(|i| grid.iter().map(|r| r.get(i).map_or(0, |c| c.chars().count())).max().unwrap_or(0)).collect();
    let text = grid
        .iter()
        .map(|row| {
            let cells: Vec<String> = row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
            format!("│ {} │", cells.join(" │ "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    print(call, &text)?;
    Ok(Value::Undefined)
}
