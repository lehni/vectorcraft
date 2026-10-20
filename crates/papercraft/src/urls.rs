//! The browser's `URL` and `URLSearchParams` for scripts outside a browser, on the `url` crate,
//! which follows the same WHATWG URL standard. A URL keeps its `href`; its other parts are
//! properties that read and change it. `url.searchParams` and the URL stay in step both ways.

use crate::{Call, Engine, Error, HostFunction, Runtime, Value, define_accessor, method, object};

/// What the classes share: `Object`, and their prototypes.
pub(crate) struct Classes<H> {
    object: H,
    url: H,
    params: H,
}

/// The parts of a URL scripts read and set, as `(name, getter, setter)`.
type Part<H> = (&'static str, HostFunction<H>, Option<HostFunction<H>>);

fn parts<H: Clone>() -> Vec<Part<H>> {
    vec![
        (
            "href",
            |call, this, _| Ok(Value::String(parsed(call, this)?.to_string())),
            Some(|call, this, args| {
                let href = text(call, args.first())?;
                let url = url::Url::parse(&href).map_err(|e| Error::Type(format!("invalid URL '{href}': {e}")))?;
                store(call, this, &url)
            }),
        ),
        ("origin", |call, this, _| Ok(Value::String(parsed(call, this)?.origin().ascii_serialization())), None),
        (
            "protocol",
            |call, this, _| Ok(Value::String(format!("{}:", parsed(call, this)?.scheme()))),
            Some(|call, this, args| {
                let protocol = text(call, args.first())?;
                change(call, this, |url| {
                    // Browsers ignore a scheme the URL can't take.
                    let _ = url.set_scheme(protocol.trim_end_matches(':'));
                })
            }),
        ),
        (
            "username",
            |call, this, _| Ok(Value::String(parsed(call, this)?.username().to_owned())),
            Some(|call, this, args| {
                let username = text(call, args.first())?;
                change(call, this, |url| {
                    let _ = url.set_username(&username);
                })
            }),
        ),
        (
            "password",
            |call, this, _| Ok(Value::String(parsed(call, this)?.password().unwrap_or_default().to_owned())),
            Some(|call, this, args| {
                let password = text(call, args.first())?;
                change(call, this, |url| {
                    let _ = url.set_password(Some(&password).filter(|p| !p.is_empty()).map(String::as_str));
                })
            }),
        ),
        (
            "host",
            |call, this, _| {
                let url = parsed(call, this)?;
                let host = url.host_str().unwrap_or_default();
                Ok(Value::String(match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_owned(),
                }))
            },
            Some(|call, this, args| {
                let host = text(call, args.first())?;
                change(call, this, |url| {
                    let (name, port) = match host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())) {
                        Some((name, port)) => (name, port.parse().ok()),
                        None => (host.as_str(), None),
                    };
                    if url.set_host(Some(name)).is_ok() && port.is_some() {
                        let _ = url.set_port(port);
                    }
                })
            }),
        ),
        (
            "hostname",
            |call, this, _| Ok(Value::String(parsed(call, this)?.host_str().unwrap_or_default().to_owned())),
            Some(|call, this, args| {
                let hostname = text(call, args.first())?;
                change(call, this, |url| {
                    let _ = url.set_host(Some(&hostname));
                })
            }),
        ),
        (
            "port",
            |call, this, _| Ok(Value::String(parsed(call, this)?.port().map(|p| p.to_string()).unwrap_or_default())),
            Some(|call, this, args| {
                let port = text(call, args.first())?;
                change(call, this, |url| {
                    let _ = url.set_port(if port.is_empty() { None } else { port.parse().ok() });
                })
            }),
        ),
        (
            "pathname",
            |call, this, _| Ok(Value::String(parsed(call, this)?.path().to_owned())),
            Some(|call, this, args| {
                let path = text(call, args.first())?;
                change(call, this, |url| url.set_path(&path))
            }),
        ),
        (
            "search",
            |call, this, _| Ok(Value::String(parsed(call, this)?.query().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default())),
            Some(|call, this, args| {
                let search = text(call, args.first())?;
                change(call, this, |url| {
                    let query = search.strip_prefix('?').unwrap_or(&search);
                    url.set_query(Some(query).filter(|q| !q.is_empty()));
                })
            }),
        ),
        (
            "hash",
            |call, this, _| Ok(Value::String(parsed(call, this)?.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default())),
            Some(|call, this, args| {
                let hash = text(call, args.first())?;
                change(call, this, |url| {
                    let fragment = hash.strip_prefix('#').unwrap_or(&hash);
                    url.set_fragment(Some(fragment).filter(|f| !f.is_empty()));
                })
            }),
        ),
        ("searchParams", search_params, None),
    ]
}

/// The methods of `URLSearchParams`.
fn params_methods<H: Clone>() -> Vec<(&'static str, HostFunction<H>)> {
    vec![
        ("append", |call, this, args| {
            let (name, value) = (text(call, args.first())?, text(call, args.get(1))?);
            edit(call, this, |list| list.push((name, value)))
        }),
        ("delete", |call, this, args| {
            let name = text(call, args.first())?;
            let value = match args.get(1) {
                Some(Value::Undefined) | None => None,
                value => Some(text(call, value)?),
            };
            edit(call, this, |list| list.retain(|(n, v)| *n != name || value.as_ref().is_some_and(|value| v != value)))
        }),
        ("get", |call, this, args| {
            let name = text(call, args.first())?;
            Ok(list(call, this)?.into_iter().find(|(n, _)| *n == name).map_or(Value::Null, |(_, v)| Value::String(v)))
        }),
        ("getAll", |call, this, args| {
            let name = text(call, args.first())?;
            let values: Vec<_> = list(call, this)?.into_iter().filter(|(n, _)| *n == name).map(|(_, v)| Value::String(v)).collect();
            Ok(Value::Js(call.new_array(&values)?))
        }),
        ("has", |call, this, args| {
            let name = text(call, args.first())?;
            Ok(Value::Bool(list(call, this)?.iter().any(|(n, _)| *n == name)))
        }),
        ("set", |call, this, args| {
            let (name, value) = (text(call, args.first())?, text(call, args.get(1))?);
            edit(call, this, |list| {
                let at = list.iter().position(|(n, _)| *n == name);
                list.retain(|(n, _)| *n != name);
                let at = at.unwrap_or(list.len()).min(list.len());
                list.insert(at, (name, value));
            })
        }),
        ("sort", |call, this, _| edit(call, this, |list| list.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16())))),
        ("toString", |call, this, _| Ok(Value::String(serialize(&list(call, this)?)))),
        ("forEach", |call, this, args| {
            let Some(Value::Js(callback)) = args.first() else {
                return Err(Error::Script("URLSearchParams.forEach: the callback is not a function".into()));
            };
            let receiver = args.get(1).cloned().unwrap_or(Value::Undefined);
            for (name, value) in list(call, this)? {
                call.call(callback, &receiver, &[Value::String(value), Value::String(name), this.clone()])?;
            }
            Ok(Value::Undefined)
        }),
        ("entries", |call, this, _| {
            let pairs = list(call, this)?
                .into_iter()
                .map(|(n, v)| call.new_array(&[Value::String(n), Value::String(v)]).map(Value::Js))
                .collect::<Result<Vec<_>, _>>()?;
            iterate(call, &pairs)
        }),
        ("keys", |call, this, _| {
            let names: Vec<_> = list(call, this)?.into_iter().map(|(n, _)| Value::String(n)).collect();
            iterate(call, &names)
        }),
        ("values", |call, this, _| {
            let values: Vec<_> = list(call, this)?.into_iter().map(|(_, v)| Value::String(v)).collect();
            iterate(call, &values)
        }),
    ]
}

/// Sets `URL` and `URLSearchParams` on `global`, unless it has them.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let url_class = runtime.engine.constructor("URL", construct_url)?;
    let params_class = runtime.engine.constructor("URLSearchParams", construct_params)?;
    let parts = parts().into_iter().map(|(name, get, set)| {
        let get = runtime.engine.function(name, get)?;
        let set = set.map(|set| runtime.engine.function(name, set)).transpose()?;
        Ok((name, get, set))
    });
    let parts = parts.collect::<Result<Vec<_>, Error>>()?;
    let to_string = runtime.engine.function("toString", |call, this, _| Ok(Value::String(parsed(call, this)?.to_string())))?;
    let can_parse = runtime.engine.function("canParse", |call, _, args| Ok(Value::Bool(parse(call, args.first(), args.get(1)).is_ok())))?;
    let methods = params_methods().into_iter().map(|(n, f)| Ok((n, runtime.engine.function(n, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let size = runtime.engine.function("size", |call, this, _| Ok(Value::Number(list(call, this)?.len() as f64)))?;
    let classes = runtime.engine.with_call(|call| {
        if !matches!(call.get(global, "URL")?, Value::Undefined) {
            return Ok(None);
        }
        let url = call.new_object()?;
        for (name, get, set) in parts {
            define_accessor(call, global, &url, name, get, set)?;
        }
        call.set(&url, "toString", &Value::Js(to_string.clone()))?;
        call.set(&url, "toJSON", &Value::Js(to_string))?;
        call.set(&url_class, "prototype", &Value::Js(url.clone()))?;
        call.set(&url_class, "canParse", &Value::Js(can_parse))?;
        call.set(global, "URL", &Value::Js(url_class))?;
        let params = call.new_object()?;
        let reflect = object(call, global, "Reflect")?;
        let symbol = object(call, global, "Symbol")?;
        let iterator = call.get(&symbol, "iterator")?;
        for (name, function) in &methods {
            call.set(&params, name, &Value::Js(function.clone()))?;
        }
        if let Some((_, entries)) = methods.iter().find(|(n, _)| *n == "entries") {
            method(call, &reflect, "set", &[Value::Js(params.clone()), iterator, Value::Js(entries.clone())])?;
        }
        define_accessor(call, global, &params, "size", size, None)?;
        call.set(&params_class, "prototype", &Value::Js(params.clone()))?;
        call.set(global, "URLSearchParams", &Value::Js(params_class))?;
        Ok(Some(Classes { object: object(call, global, "Object")?, url, params }))
    })?;
    runtime.host.borrow_mut().urls = classes;
    Ok(())
}

fn text<H>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<String, Error> {
    match value {
        None | Some(Value::Undefined) => Ok(String::new()),
        Some(value) => call.to_string(value),
    }
}

/// `url` against `base`, as `new URL(url, base)` parses it.
fn parse<H>(call: &mut dyn Call<H>, url: Option<&Value<H>>, base: Option<&Value<H>>) -> Result<url::Url, Error> {
    let url = text(call, url)?;
    let parsed = match base {
        None | Some(Value::Undefined) => url::Url::parse(&url),
        Some(base) => {
            let base = call.to_string(base)?;
            url::Url::parse(&base).and_then(|base| base.join(&url))
        }
    };
    parsed.map_err(|e| Error::Type(format!("invalid URL '{url}': {e}")))
}

/// A new object with the prototype `prototype` picks.
fn create<H: Clone>(call: &mut dyn Call<H>, prototype: impl FnOnce(&Classes<H>) -> &H) -> Result<H, Error> {
    let host = call.host()?;
    let (object_class, prototype) = {
        let host = host.borrow();
        let classes = host.urls.as_ref().ok_or_else(|| Error::Script("URL is not set up".into()))?;
        (classes.object.clone(), prototype(classes).clone())
    };
    match method(call, &object_class, "create", &[Value::Js(prototype)])? {
        Value::Js(made) => Ok(made),
        _ => Err(Error::Script("Object.create made nothing".into())),
    }
}

/// `new URL(url, base)`.
fn construct_url<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let url = parse(call, args.first(), args.get(1))?;
    let made = create(call, |c| &c.url)?;
    call.set(&made, "_href", &Value::String(url.to_string()))?;
    Ok(Value::Js(made))
}

/// The URL `this` is.
fn parsed<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<url::Url, Error> {
    let Value::Js(made) = this else {
        return Err(Error::Script("not a URL".into()));
    };
    match call.get(made, "_href")? {
        Value::String(href) => url::Url::parse(&href).map_err(|e| Error::Script(e.to_string())),
        _ => Err(Error::Script("not a URL".into())),
    }
}

/// Makes `url` the URL `this` is, and its `searchParams` follow.
fn store<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, url: &url::Url) -> Result<Value<H>, Error> {
    if let Value::Js(made) = this {
        call.set(made, "_href", &Value::String(url.to_string()))?;
        if let Value::Js(params) = call.get(made, "_params")? {
            let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            store_list(call, &params, &pairs)?;
        }
    }
    Ok(Value::Undefined)
}

/// Changes the URL `this` is with `f`.
fn change<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut url::Url)) -> Result<Value<H>, Error> {
    let mut url = parsed(call, this)?;
    f(&mut url);
    store(call, this, &url)
}

/// `url.searchParams`: one `URLSearchParams` per URL, which changes it.
fn search_params<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(made) = this else {
        return Err(Error::Script("not a URL".into()));
    };
    if let Value::Js(params) = call.get(made, "_params")? {
        return Ok(Value::Js(params));
    }
    let pairs: Vec<(String, String)> = parsed(call, this)?.query_pairs().into_owned().collect();
    let params = create(call, |c| &c.params)?;
    store_list(call, &params, &pairs)?;
    call.set(&params, "_url", this)?;
    call.set(made, "_params", &Value::Js(params.clone()))?;
    Ok(Value::Js(params))
}

/// `new URLSearchParams(init)`: from a query string (with or without `?`), `[[name, value], …]`,
/// `{ name: value }` or another `URLSearchParams`.
fn construct_params<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let pairs: Vec<(String, String)> = match args.first() {
        None | Some(Value::Undefined | Value::Null) => Vec::new(),
        Some(Value::Js(init)) if matches!(call.get(init, "_list")?, Value::String(_)) => list(call, &Value::Js(init.clone()))?,
        Some(Value::Js(init)) => {
            let globals = crate::dialog::globals(call)?;
            match crate::dialog::to_json(call, &globals, &Value::Js(init.clone()))? {
                serde_json::Value::Array(pairs) => pairs.iter().filter_map(|p| Some((json_text(p.get(0)?), json_text(p.get(1)?)))).collect(),
                serde_json::Value::Object(map) => map.iter().map(|(n, v)| (n.clone(), json_text(v))).collect(),
                _ => Vec::new(),
            }
        }
        Some(init) => {
            let query = call.to_string(init)?;
            url::form_urlencoded::parse(query.strip_prefix('?').unwrap_or(&query).as_bytes()).into_owned().collect()
        }
    };
    let params = create(call, |c| &c.params)?;
    store_list(call, &params, &pairs)?;
    Ok(Value::Js(params))
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The pairs of the `URLSearchParams` `this` is.
fn list<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<(String, String)>, Error> {
    let Value::Js(params) = this else {
        return Err(Error::Script("not a URLSearchParams object".into()));
    };
    match call.get(params, "_list")? {
        Value::String(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
        _ => Err(Error::Script("not a URLSearchParams object".into())),
    }
}

fn store_list<H: Clone>(call: &mut dyn Call<H>, params: &H, pairs: &[(String, String)]) -> Result<(), Error> {
    let json = serde_json::to_string(pairs).map_err(|e| Error::Script(e.to_string()))?;
    call.set(params, "_list", &Value::String(json))
}

/// Changes the pairs with `f`, and the URL they belong to.
fn edit<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Vec<(String, String)>)) -> Result<Value<H>, Error> {
    let mut pairs = list(call, this)?;
    f(&mut pairs);
    let Value::Js(params) = this else {
        return Ok(Value::Undefined);
    };
    store_list(call, params, &pairs)?;
    if let Value::Js(owner) = call.get(params, "_url")? {
        let owner = Value::Js(owner);
        let mut url = parsed(call, &owner)?;
        url.set_query(Some(serialize(&pairs)).filter(|q| !q.is_empty()).as_deref());
        if let Value::Js(made) = &owner {
            call.set(made, "_href", &Value::String(url.to_string()))?;
        }
    }
    Ok(Value::Undefined)
}

/// The pairs as `application/x-www-form-urlencoded`.
fn serialize(pairs: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

/// An iterator over `values`.
fn iterate<H: Clone>(call: &mut dyn Call<H>, values: &[Value<H>]) -> Result<Value<H>, Error> {
    let array = call.new_array(values)?;
    method(call, &array, "values", &[])
}
