//! mdio Arrow/DuckDB facade v1a (HS-only).
//!
//! md text -> Snapshot -> FacadeCore -> DuckDB table functions.
//! Default md render is deliberately absent in v1a; it waits for R0.

#[cfg(feature = "calibration")]
mod calibration;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use duckdb_loadable_macros::duckdb_entrypoint_c_api_v2;
use duckdb_neo::DuckDBType;
use duckdb_neo::bind_arguments::BindArgument;
use duckdb_neo::connection::Context;
use duckdb_neo::data_chunk::{DataChunkRef, VectorCollection};
use duckdb_neo::extension::Extension;
use duckdb_neo::scalar::{ScalarCallbacks, ScalarFunctionBuilder};
use duckdb_neo::signature::{FunctionParameter, SignatureBuilder};
use duckdb_neo::table_function::{BindFunctionHandle, ExecColumnInfo, InitColumnData, OrderPreservation, TableFunctionBuilder, TableFunctionCallbacks};
use duckdb_neo::vector::{Unknown, Vector};

use mdio_core::projection::Snapshot;
use mdio_core::arrow::facade;
use mdio_core::arrow::state::{FaceRef, FaceSrc};
use facade::{DescRow, FacadeBase, FacadeRelations, SCHEMA_VERSION};

static SESSIONS_DATA: OnceLock<Mutex<HashMap<u64, Arc<SessionData>>>> = OnceLock::new();
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

struct OpenDoc;

impl ScalarCallbacks for OpenDoc {
    type BindData = ();
    type InitData = ();

    fn exec(
        &self,
        _bind_data: Option<&Self::BindData>,
        _init_data: Option<&mut Self::InitData>,
        _context: &Context,
        input: &mut VectorCollection,
        output: Vector<'_, Unknown>,
    ) -> duckdb_neo::Result<()> {
        let text_vec = input.get_vector_at::<String>(0)?;
        let text = text_vec.reader()?.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_open: text is NULL"))?
            .to_string();
        let snap = Arc::new(mdio_core::projection::project_text(&text));
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let data = SessionData::new(snap)?;
        SESSIONS_DATA.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().insert(id, data);
        let mut out = output.cast::<i64>()?;
        out.set_size(1)?;
        out.writer()?.set(0, Some(id as i64))?;
        Ok(())
    }
}


struct OpenSnapshot;

impl ScalarCallbacks for OpenSnapshot {
    type BindData = ();
    type InitData = ();

    fn exec(
        &self,
        _bind_data: Option<&Self::BindData>,
        _init_data: Option<&mut Self::InitData>,
        _context: &Context,
        input: &mut VectorCollection,
        output: Vector<'_, Unknown>,
    ) -> duckdb_neo::Result<()> {
        let blob = input.get_vector_at::<duckdb_neo::types::BlobValue>(0)?;
        let reader = blob.reader()?;
        let bytes = reader.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_open_snapshot: blob is NULL"))?;
        let snap = mdio_core::arrow::blob::decode(bytes)
            .map_err(|e| duckdb_neo::Error::api_error(e))?;
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let data = SessionData::new(Arc::new(snap))?;
        SESSIONS_DATA.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().insert(id, data);
        let mut out = output.cast::<i64>()?;
        out.set_size(1)?;
        out.writer()?.set(0, Some(id as i64))?;
        Ok(())
    }
}

struct CloseDoc;
impl ScalarCallbacks for CloseDoc {
    type BindData = ();
    type InitData = ();
    fn exec(&self, _b: Option<&Self::BindData>, _i: Option<&mut Self::InitData>, _c: &Context, input: &mut VectorCollection, output: Vector<'_, Unknown>) -> duckdb_neo::Result<()> {
        let sid = input.get_vector_at::<i64>(0)?.reader()?.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_close: null session"))?;
        let removed = SESSIONS_DATA.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().remove(&(*sid as u64)).is_some();
        let mut out = output.cast::<bool>()?;
        out.set_size(1)?;
        out.writer()?.set(0, Some(removed))?;
        Ok(())
    }
}

struct RenderAll;
impl ScalarCallbacks for RenderAll {
    type BindData = ();
    type InitData = ();
    fn exec(&self, _b: Option<&Self::BindData>, _i: Option<&mut Self::InitData>, _c: &Context, input: &mut VectorCollection, output: Vector<'_, Unknown>) -> duckdb_neo::Result<()> {
        let sid = input.get_vector_at::<i64>(0)?.reader()?.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_render_all: null session"))?;
        let data = session_data(*sid)?;
        let text = data.render_all();
        let mut out = output.cast::<String>()?;
        out.set_size(1)?;
        out.writer()?.set(0, Some(text.as_str()))?;
        Ok(())
    }
}

struct RenderSelected;
impl ScalarCallbacks for RenderSelected {
    type BindData = ();
    type InitData = ();
    fn exec(&self, _b: Option<&Self::BindData>, _i: Option<&mut Self::InitData>, _c: &Context, input: &mut VectorCollection, output: Vector<'_, Unknown>) -> duckdb_neo::Result<()> {
        let sid = input.get_vector_at::<i64>(0)?.reader()?.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_render_selected: null session"))?;
        let spec = input.get_vector_at::<String>(1)?.reader()?.get(0)?
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_render_selected: null nodes JSON"))?;
        let ids: Vec<i64> = serde_json::from_str(spec)
            .map_err(|e| duckdb_neo::Error::api_error(format!("mdio_render_selected: bad nodes JSON: {e}")))?;
        let data = session_data(*sid)?;
        let text = data.render_selected(&ids).map_err(duckdb_neo::Error::api_error)?;
        let mut out = output.cast::<String>()?;
        out.set_size(1)?;
        out.writer()?.set(0, Some(text.as_str()))?;
        Ok(())
    }
}

fn session_data(id: i64) -> duckdb_neo::Result<Arc<SessionData>> {
    SESSIONS_DATA.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap()
        .get(&(id as u64)).cloned()
        .ok_or_else(|| duckdb_neo::Error::api_error(format!("mdio: unknown session {id}")))
}

fn session_arg(arguments: &[BindArgument], name: &str) -> duckdb_neo::Result<i64> {
    arguments.first().and_then(|a| a.value.as_ref()).and_then(|v| v.get::<i64>().ok().flatten())
        .ok_or_else(|| duckdb_neo::Error::api_error(format!("{name}: session id must be a constant BIGINT")))
}

struct FaceBindData { data: Arc<SessionData> }
struct FaceGlobalState { projected: Vec<usize> }

struct MdioFaces;

impl TableFunctionCallbacks for MdioFaces {
    type BindData = FaceBindData;
    type GlobalState = FaceGlobalState;
    type LocalState = usize;

    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_faces")?;
        let data = session_data(sid)?;
        for name in ["node", "level", "frame_ordinal", "section", "can_read", "can_write"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        for name in ["role", "type"] {
            h.add_result_column(name, &String::logical_type(context)?)?;
        }
        for name in ["raw", "mark", "tag", "content", "container"] {
            h.add_result_column(name, &String::logical_type(context)?)?;
        }
        Ok(FaceBindData { data })
    }

    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(FaceGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }

    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_faces: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_faces: no local state"))?;
        let f = b.data.base()?;
        let n = f.node_rows.len();
        if *off >= n {
            for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; }
            return Ok(());
        }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            match orig {
                0 | 1 | 2 | 3 | 4 | 5 => {
                    let mut v = out.get_vector_at_mut::<i64>(j)?;
                    v.set_size(take)?;
                    let mut w = v.writer()?;
                    for i in 0..take {
                        let r = &f.node_rows[*off + i];
                        let val = match orig {
                            0 => r.node as i64,
                            1 => r.level as i64,
                            2 => r.frame_ordinal as i64,
                            3 => r.section.map(|x| x as i64).unwrap_or(-1),
                            4 => r.can_read as i64,
                            _ => r.can_write as i64,
                        };
                        w.set(i, Some(val))?;
                    }
                }
                6 | 7 => {
                    let mut v = out.get_vector_at_mut::<String>(j)?;
                    v.set_size(take)?;
                    let mut w = v.writer()?;
                    for i in 0..take {
                        let r = &f.node_rows[*off + i];
                        w.set(i, Some(if orig == 6 { r.role } else { r.type_name }))?;
                    }
                }
                8..=12 => {
                    write_face_column(&mut out, j, take, *off, &b.data, orig - 8)?;
                }
                _ => return Err(duckdb_neo::Error::api_error(format!("mdio_faces: unknown column {orig}"))),
            }
        }
        *off += take;
        Ok(())
    }
}

struct MdioNodes;

impl TableFunctionCallbacks for MdioNodes {
    type BindData = FaceBindData;
    type GlobalState = FaceGlobalState;
    type LocalState = usize;

    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_nodes")?;
        let data = session_data(sid)?;
        for name in ["node", "level", "frame_ordinal", "section", "can_read", "can_write"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        for name in ["role", "type"] {
            h.add_result_column(name, &String::logical_type(context)?)?;
        }
        Ok(FaceBindData { data })
    }

    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(FaceGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }

    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_nodes: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_nodes: no local state"))?;
        let f = b.data.base()?;
        let n = f.node_rows.len();
        if *off >= n {
            for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; }
            return Ok(());
        }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            match orig {
                0 | 1 | 2 | 3 | 4 | 5 => {
                    let mut v = out.get_vector_at_mut::<i64>(j)?;
                    v.set_size(take)?;
                    let mut w = v.writer()?;
                    for i in 0..take {
                        let r = &f.node_rows[*off + i];
                        let val = match orig {
                            0 => r.node as i64,
                            1 => r.level as i64,
                            2 => r.frame_ordinal as i64,
                            3 => r.section.map(|x| x as i64).unwrap_or(-1),
                            4 => r.can_read as i64,
                            _ => r.can_write as i64,
                        };
                        w.set(i, Some(val))?;
                    }
                }
                6 | 7 => {
                    let mut v = out.get_vector_at_mut::<String>(j)?;
                    v.set_size(take)?;
                    let mut w = v.writer()?;
                    for i in 0..take {
                        let r = &f.node_rows[*off + i];
                        w.set(i, Some(if orig == 6 { r.role } else { r.type_name }))?;
                    }
                }
                _ => return Err(duckdb_neo::Error::api_error(format!("mdio_nodes: unknown column {orig}"))),
            }
        }
        *off += take;
        Ok(())
    }
}

struct EdgeBindData { data: Arc<SessionData> }
struct EdgeGlobalState { projected: Vec<usize> }
struct Edges;

impl TableFunctionCallbacks for Edges {
    type BindData = EdgeBindData;
    type GlobalState = EdgeGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_edges")?;
        let data = session_data(sid)?;
        for name in ["parent", "child", "pos"] { h.add_result_column(name, &i64::logical_type(context)?)?; }
        Ok(EdgeBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(EdgeGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_edges: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_edges: no local state"))?;
        let f = b.data.relations()?;
        let n = f.edges.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            let mut v = out.get_vector_at_mut::<i64>(j)?;
            v.set_size(take)?;
            let mut w = v.writer()?;
            for i in 0..take {
                let e = f.edges[*off + i];
                w.set(i, Some(match orig { 0 => e.parent as i64, 1 => e.child as i64, _ => e.pos as i64 }))?;
            }
        }
        *off += take;
        Ok(())
    }
}

struct RootBindData { data: Arc<SessionData> }
struct RootGlobalState { projected: Vec<usize> }
struct Roots;

impl TableFunctionCallbacks for Roots {
    type BindData = RootBindData;
    type GlobalState = RootGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_roots")?;
        let data = session_data(sid)?;
        for name in ["node", "pos"] { h.add_result_column(name, &i64::logical_type(context)?)?; }
        Ok(RootBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(RootGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_roots: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_roots: no local state"))?;
        let f = b.data.relations()?;
        let n = f.roots.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            let mut v = out.get_vector_at_mut::<i64>(j)?;
            v.set_size(take)?;
            let mut w = v.writer()?;
            for i in 0..take {
                let r = f.roots[*off + i];
                w.set(i, Some(if orig == 0 { r.node as i64 } else { r.pos as i64 }))?;
            }
        }
        *off += take;
        Ok(())
    }
}

struct DescBindData { data: Arc<SessionData> }
struct DescGlobalState { projected: Vec<usize> }
struct Descendants;

impl TableFunctionCallbacks for Descendants {
    type BindData = DescBindData;
    type GlobalState = DescGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_descendants")?;
        let data = session_data(sid)?;
        for name in ["ancestor", "descendant", "depth"] { h.add_result_column(name, &i64::logical_type(context)?)?; }
        Ok(DescBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(DescGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_descendants: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_descendants: no local state"))?;
        let desc = b.data.descendants()?;
        let n = desc.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            let mut v = out.get_vector_at_mut::<i64>(j)?;
            v.set_size(take)?;
            let mut w = v.writer()?;
            for i in 0..take {
                let r = desc[*off + i];
                w.set(i, Some(match orig { 0 => r.ancestor as i64, 1 => r.descendant as i64, _ => r.depth as i64 }))?;
            }
        }
        *off += take;
        Ok(())
    }
}

struct SectionBindData { data: Arc<SessionData> }
struct SectionGlobalState { projected: Vec<usize> }
struct Sections;

impl TableFunctionCallbacks for Sections {
    type BindData = SectionBindData;
    type GlobalState = SectionGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_sections")?;
        let data = session_data(sid)?;
        for name in ["section", "parent_section", "level", "start", "end", "depth"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        h.add_result_column("in_list", &bool::logical_type(context)?)?;
        h.add_result_column("header_node", &i64::logical_type(context)?)?;
        Ok(SectionBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(SectionGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_sections: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_sections: no local state"))?;
        let f = b.data.base()?;
        let n = f.sections.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            if orig == 6 {
                let mut v = out.get_vector_at_mut::<bool>(j)?;
                v.set_size(take)?;
                let mut w = v.writer()?;
                for i in 0..take { w.set(i, Some(f.sections[*off + i].in_list))?; }
            } else {
                let mut v = out.get_vector_at_mut::<i64>(j)?;
                v.set_size(take)?;
                let mut w = v.writer()?;
                for i in 0..take {
                    let r = f.sections[*off + i];
                    let val = match orig {
                        0 => Some(r.section as i64),
                        1 => r.parent_section.map(|x| x as i64),
                        2 => Some(r.level as i64),
                        3 => Some(r.start as i64),
                        4 => Some(r.end as i64),
                        5 => Some(r.depth as i64),
                        7 => Some(r.header_node as i64),
                        _ => None,
                    };
                    w.set(i, val)?;
                }
            }
        }
        *off += take;
        Ok(())
    }
}

struct MemberBindData { data: Arc<SessionData> }
struct MemberGlobalState { projected: Vec<usize> }
struct SectionMembers;

impl TableFunctionCallbacks for SectionMembers {
    type BindData = MemberBindData;
    type GlobalState = MemberGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_section_member")?;
        let data = session_data(sid)?;
        h.add_result_column("node", &i64::logical_type(context)?)?;
        h.add_result_column("section", &i64::logical_type(context)?)?;
        Ok(MemberBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(MemberGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_section_member: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_section_member: no local state"))?;
        let f = b.data.base()?;
        let n = f.members.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            let mut v = out.get_vector_at_mut::<i64>(j)?;
            v.set_size(take)?;
            let mut w = v.writer()?;
            for i in 0..take {
                let m = f.members[*off + i];
                w.set(i, Some(if orig == 0 { m.node as i64 } else { m.section as i64 }))?;
            }
        }
        *off += take;
        Ok(())
    }
}

struct SpanBindData { data: Arc<SessionData> }
struct SpanGlobalState { projected: Vec<usize> }
struct Spans;

impl TableFunctionCallbacks for Spans {
    type BindData = SpanBindData;
    type GlobalState = SpanGlobalState;
    type LocalState = usize;
    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let sid = session_arg(&arguments, "mdio_span")?;
        let data = session_data(sid)?;
        for name in ["node", "start_line", "end_line", "byte_start", "byte_end"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        Ok(SpanBindData { data })
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(SpanGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(0)) }
    fn exec(&self, b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_span: no global state"))?;
        let off = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_span: no local state"))?;
        let f = b.data.base()?;
        let n = f.spans.len();
        if *off >= n { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        let take = (n - *off).min(out.capacity()?.max(1).min(1024));
        for (j, &orig) in g.projected.iter().enumerate() {
            let mut v = out.get_vector_at_mut::<i64>(j)?;
            v.set_size(take)?;
            let mut w = v.writer()?;
            for i in 0..take {
                let r = f.spans[*off + i];
                let val = match orig {
                    0 => Some(r.node as i64),
                    1 => Some(r.start_line as i64),
                    2 => Some(r.end_line as i64),
                    3 => Some(r.byte_start as i64),
                    _ => Some(r.byte_end as i64),
                };
                w.set(i, val)?;
            }
        }
        *off += take;
        Ok(())
    }
}

struct Meta;
struct MetaGlobalState { projected: Vec<usize> }
struct MetaLocalState { done: bool }

impl TableFunctionCallbacks for Meta {
    type BindData = ();
    type GlobalState = MetaGlobalState;
    type LocalState = MetaLocalState;
    fn bind(&self, context: &Context, _arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        h.add_result_column("topology", &String::logical_type(context)?)?;
        h.add_result_column("schema_version", &i64::logical_type(context)?)?;
        Ok(())
    }
    fn init_global_state(&self, _b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        Ok((Some(MetaGlobalState { projected }), None))
    }
    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> { Ok(Some(MetaLocalState { done: false })) }
    fn exec(&self, _b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_meta: no global state"))?;
        let st = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_meta: no local state"))?;
        if st.done { for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; } return Ok(()); }
        st.done = true;
        for (j, &orig) in g.projected.iter().enumerate() {
            if orig == 0 {
                let mut v = out.get_vector_at_mut::<String>(j)?;
                v.set_size(1)?;
                v.writer()?.set(0, Some("hs"))?;
            } else {
                let mut v = out.get_vector_at_mut::<i64>(j)?;
                v.set_size(1)?;
                v.writer()?.set(0, Some(SCHEMA_VERSION as i64))?;
            }
        }
        Ok(())
    }
}

struct SessionData {
    facade: Arc<mdio_core::arrow::state::FacadeState>,
}

impl SessionData {
    fn new(snap: Arc<Snapshot>) -> duckdb_neo::Result<Arc<Self>> {
        let facade = mdio_core::arrow::state::FacadeState::new(snap)
            .map_err(|e| duckdb_neo::Error::api_error(e.to_string()))?;
        Ok(Arc::new(Self { facade }))
    }
    fn base(&self) -> duckdb_neo::Result<Arc<FacadeBase>> {
        self.facade.base().map_err(duckdb_neo::Error::api_error)
    }
    fn relations(&self) -> duckdb_neo::Result<Arc<FacadeRelations>> {
        self.facade.relations().map_err(duckdb_neo::Error::api_error)
    }
    fn descendants(&self) -> duckdb_neo::Result<Arc<Vec<DescRow>>> {
        self.facade.descendants().map_err(duckdb_neo::Error::api_error)
    }
    fn raw(&self) -> &mdio_core::arrow::state::FaceColumn { self.facade.raw() }
    fn mark(&self) -> &mdio_core::arrow::state::FaceColumn { self.facade.mark() }
    fn tag(&self) -> &mdio_core::arrow::state::FaceColumn { self.facade.tag() }
    fn content(&self) -> &mdio_core::arrow::state::ContentCol { self.facade.content() }
    fn container(&self) -> &mdio_core::arrow::state::FaceColumn { self.facade.container() }
    fn render_all(&self) -> String { self.facade.render_all() }
    fn render_selected(&self, ids: &[i64]) -> Result<String, String> {
        self.facade.render_selected(ids)
    }
}

fn write_face_column(output: &mut DataChunkRef<'_>, j: usize, take: usize, start: usize, data: &SessionData, face: usize) -> duckdb_neo::Result<()> {
    let (refs, fallback, content_bytes): (&[FaceRef], &[u8], Option<&[u8]>) = match face {
        0 => { let c = data.raw(); (&c.refs, &c.fallback, None) }
        1 => { let c = data.mark(); (&c.refs, &c.fallback, None) }
        2 => { let c = data.tag(); (&c.refs, &c.fallback, None) }
        3 => { let c = data.content(); (&c.refs, &[], Some(&c.bytes)) }
        4 => { let c = data.container(); (&c.refs, &c.fallback, None) }
        _ => return Err(duckdb_neo::Error::api_error("mdio_faces: bad face column")),
    };
    let chunk_ptrs: Vec<*const u8> = data.facade.snap.rope.chunks().map(|c| c.as_ptr()).collect();
    let mut v = output.get_vector_at_mut::<String>(j)?;
    v.set_size(take)?;
    let mut w = v.writer()?;
    for i in 0..take {
        let r = refs[start + i];
        let bytes: &[u8] = match r.src {
            FaceSrc::Chunk(ci) => {
                let ptr = chunk_ptrs[ci as usize];
                unsafe { std::slice::from_raw_parts(ptr.add(r.off as usize), r.len as usize) }
            }
            FaceSrc::Fallback => &fallback[r.off as usize..(r.off + r.len) as usize],
            FaceSrc::Content => &content_bytes.expect("content bytes")[r.off as usize..(r.off + r.len) as usize],
        };
        let s = std::str::from_utf8(bytes).map_err(|e| duckdb_neo::Error::api_error(format!("mdio_faces: utf8: {e}")))?;
        w.set(i, Some(s))?;
    }
    Ok(())
}

// --- prototype: one parallel table function over many sessions (additive) ---
#[derive(serde::Deserialize)]
struct SessionSpec { path: String, sid: u64 }

struct AllFacesBindData { sessions: Arc<Vec<(String, Arc<SessionData>)>> }
struct AllFacesGlobal { sessions: Arc<Vec<(String, Arc<SessionData>)>>, projected: Vec<usize>, next: AtomicUsize }
struct AllFacesLocal { cur: Option<(usize, usize)> }

struct MdioFacesAll;

impl TableFunctionCallbacks for MdioFacesAll {
    type BindData = AllFacesBindData;
    type GlobalState = AllFacesGlobal;
    type LocalState = AllFacesLocal;

    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let raw = arguments.first().and_then(|a| a.value.as_ref())
            .and_then(|v| v.get::<String>().ok().flatten())
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_faces_all: sessions JSON must be a constant VARCHAR"))?;
        let specs: Vec<SessionSpec> = serde_json::from_str(&raw)
            .map_err(|e| duckdb_neo::Error::api_error(format!("mdio_faces_all: bad sessions JSON: {e}")))?;
        let mut sessions = Vec::with_capacity(specs.len());
        for s in specs {
            let data = session_data(s.sid as i64)?;
            sessions.push((s.path, data));
        }
        h.set_order(OrderPreservation::None)?;
        h.add_result_column("path", &String::logical_type(context)?)?;
        for name in ["node", "level", "frame_ordinal", "section", "can_read", "can_write"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        for name in ["role", "type"] {
            h.add_result_column(name, &String::logical_type(context)?)?;
        }
        for name in ["raw", "mark", "tag", "content", "container"] {
            h.add_result_column(name, &String::logical_type(context)?)?;
        }
        Ok(AllFacesBindData { sessions: Arc::new(sessions) })
    }

    fn init_global_state(&self, b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        let threads = b.sessions.len().clamp(1, 8);
        Ok((Some(AllFacesGlobal { sessions: b.sessions.clone(), projected, next: AtomicUsize::new(0) }), Some(threads)))
    }

    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> {
        Ok(Some(AllFacesLocal { cur: None }))
    }

    fn exec(&self, _b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_faces_all: no global state"))?;
        let l = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_faces_all: no local state"))?;
        loop {
            let (idx, off) = match l.cur {
                Some(x) => x,
                None => {
                    let i = g.next.fetch_add(1, Ordering::Relaxed);
                    if i >= g.sessions.len() {
                        for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; }
                        return Ok(());
                    }
                    (i, 0usize)
                }
            };
            let (path, data) = &g.sessions[idx];
            let f = data.base()?;
            let n = f.node_rows.len();
            if off >= n { l.cur = None; continue; }
            let take = (n - off).min(out.capacity()?.max(1).min(1024));
            for (j, &orig) in g.projected.iter().enumerate() {
                match orig {
                    0 => {
                        let mut v = out.get_vector_at_mut::<String>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take { w.set(i, Some(path.as_str()))?; }
                    }
                    1 | 2 | 3 | 4 | 5 | 6 => {
                        let mut v = out.get_vector_at_mut::<i64>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take {
                            let r = &f.node_rows[off + i];
                            let val = match orig {
                                1 => r.node as i64,
                                2 => r.level as i64,
                                3 => r.frame_ordinal as i64,
                                4 => r.section.map(|x| x as i64).unwrap_or(-1),
                                5 => r.can_read as i64,
                                _ => r.can_write as i64,
                            };
                            w.set(i, Some(val))?;
                        }
                    }
                    7 | 8 => {
                        let mut v = out.get_vector_at_mut::<String>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take {
                            let r = &f.node_rows[off + i];
                            w.set(i, Some(if orig == 7 { r.role } else { r.type_name }))?;
                        }
                    }
                    9..=13 => { write_face_column(&mut out, j, take, off, data.as_ref(), orig - 9)?; }
                    _ => return Err(duckdb_neo::Error::api_error(format!("mdio_faces_all: unknown column {orig}"))),
                }
            }
            l.cur = Some((idx, off + take));
            return Ok(());
        }
    }
}

// --- prototype: complete search blocks across sessions, computed in Rust, parallel ---
struct BlockRow {
    node: i64,
    section: i64,
    type_name: String,
    heading: String,
    tag: String,
    parent: Option<i64>,
    pos: Option<i64>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    text: String,
}

fn face_value<'a>(data: &'a SessionData, face: usize, idx: usize, chunk_ptrs: &[*const u8]) -> duckdb_neo::Result<&'a str> {
    let (refs, fallback, content_bytes): (&[FaceRef], &[u8], Option<&[u8]>) = match face {
        0 => { let c = data.raw(); (&c.refs, &c.fallback, None) }
        1 => { let c = data.mark(); (&c.refs, &c.fallback, None) }
        2 => { let c = data.tag(); (&c.refs, &c.fallback, None) }
        3 => { let c = data.content(); (&c.refs, &[], Some(&c.bytes)) }
        4 => { let c = data.container(); (&c.refs, &c.fallback, None) }
        _ => return Err(duckdb_neo::Error::api_error("mdio_blocks_all: bad face column")),
    };
    let r = refs[idx];
    let bytes: &[u8] = match r.src {
        FaceSrc::Chunk(ci) => {
            let ptr = chunk_ptrs[ci as usize];
            unsafe { std::slice::from_raw_parts(ptr.add(r.off as usize), r.len as usize) }
        }
        FaceSrc::Fallback => &fallback[r.off as usize..(r.off + r.len) as usize],
        FaceSrc::Content => &content_bytes.expect("content bytes")[r.off as usize..(r.off + r.len) as usize],
    };
    std::str::from_utf8(bytes).map_err(|e| duckdb_neo::Error::api_error(format!("mdio_blocks_all: utf8: {e}")))
}

fn build_blocks(data: &SessionData) -> duckdb_neo::Result<Vec<BlockRow>> {
    let f = data.base()?;
    let rel = data.relations()?;
    let n = f.node_rows.len();
    let mut row_of_node: HashMap<usize, usize> = HashMap::with_capacity(n);
    for (i, r) in f.node_rows.iter().enumerate() { row_of_node.insert(r.node as usize, i); }
    let chunk_ptrs: Vec<*const u8> = data.facade.snap.rope.chunks().map(|c| c.as_ptr()).collect();
    // own-text = content minus the content of content-bearing children (matching the SQL rule)
    let mut child_len = vec![0usize; n];
    for e in rel.edges.iter() {
        let ci = match row_of_node.get(&(e.child as usize)) { Some(v) => *v, None => continue };
        let cr = &f.node_rows[ci];
        if (cr.can_read & 8) == 0 { continue; }
        let c = face_value(data, 3, ci, &chunk_ptrs)?;
        if c.trim_matches(' ').is_empty() { continue; }
        if let Some(&pi) = row_of_node.get(&(e.parent as usize)) { child_len[pi] += c.chars().count(); }
    }
    // parent / pos: min(parent) per child with arg_min(pos)
    let mut parent_pos: Vec<Option<(i64, i64)>> = vec![None; n];
    for e in rel.edges.iter() {
        if let Some(&ci) = row_of_node.get(&(e.child as usize)) {
            let cand = (e.parent as i64, e.pos as i64);
            match parent_pos[ci] {
                None => parent_pos[ci] = Some(cand),
                Some((p, _)) if cand.0 < p => parent_pos[ci] = Some(cand),
                _ => {}
            }
        }
    }
    let mut spans: Vec<Option<(i64, i64)>> = vec![None; n];
    for s in f.spans.iter() {
        if let Some(&i) = row_of_node.get(&(s.node as usize)) { spans[i] = Some((s.start_line as i64, s.end_line as i64)); }
    }
    let mut sec_hdr: HashMap<usize, usize> = HashMap::with_capacity(f.sections.len());
    for s in f.sections.iter() { sec_hdr.insert(s.section as usize, s.header_node as usize); }
    let mut out = Vec::new();
    for (i, r) in f.node_rows.iter().enumerate() {
        if r.role == "header" { continue; }
        if (r.can_read & 8) == 0 { continue; }
        let content = face_value(data, 3, i, &chunk_ptrs)?;
        if content.trim_matches(' ').is_empty() { continue; }
        let cl = child_len[i];
        if r.type_name == "bq" && cl > 0 { continue; }
        let own: String = if cl == 0 {
            content.to_string()
        } else {
            let keep = content.chars().count().saturating_sub(cl);
            content.chars().take(keep).collect()
        };
        if own.trim_matches(' ').is_empty() { continue; }
        let heading = match r.section.and_then(|s| sec_hdr.get(&(s as usize))).and_then(|h| row_of_node.get(h)) {
            Some(&hi) => face_value(data, 2, hi, &chunk_ptrs)?.to_string(),
            None => String::new(),
        };
        let (parent, pos) = match parent_pos[i] { Some((p, q)) => (Some(p), Some(q)), None => (None, None) };
        let (start_line, end_line) = match spans[i] { Some((a, b)) => (Some(a), Some(b)), None => (None, None) };
        out.push(BlockRow {
            node: r.node as i64,
            section: r.section.map(|x| x as i64).unwrap_or(-1),
            type_name: r.type_name.to_string(),
            heading,
            tag: face_value(data, 2, i, &chunk_ptrs)?.to_string(),
            parent,
            pos,
            start_line,
            end_line,
            text: own,
        });
    }
    Ok(out)
}

// --- prototype: sections for many sessions, in parallel (for results/dedupe) ---
struct SectionsAll;
struct SectionsAllBind { sessions: Arc<Vec<(String, Arc<SessionData>)>> }
struct SectionsAllGlobal { sessions: Arc<Vec<(String, Arc<SessionData>)>>, projected: Vec<usize>, next: AtomicUsize }
struct SectionsAllLocal { cur: Option<usize>, off: usize }

impl TableFunctionCallbacks for SectionsAll {
    type BindData = SectionsAllBind;
    type GlobalState = SectionsAllGlobal;
    type LocalState = SectionsAllLocal;

    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let raw = arguments.first().and_then(|a| a.value.as_ref())
            .and_then(|v| v.get::<String>().ok().flatten())
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_sections_all: sessions JSON must be a constant VARCHAR"))?;
        let specs: Vec<SessionSpec> = serde_json::from_str(&raw)
            .map_err(|e| duckdb_neo::Error::api_error(format!("mdio_sections_all: bad sessions JSON: {e}")))?;
        let mut sessions = Vec::with_capacity(specs.len());
        for s in specs { sessions.push((s.path, session_data(s.sid as i64)?)); }
        h.set_order(OrderPreservation::None)?;
        h.add_result_column("path", &String::logical_type(context)?)?;
        for name in ["section", "parent_section", "level", "start", "end", "depth"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        h.add_result_column("in_list", &bool::logical_type(context)?)?;
        h.add_result_column("header_node", &i64::logical_type(context)?)?;
        Ok(SectionsAllBind { sessions: Arc::new(sessions) })
    }

    fn init_global_state(&self, b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        let threads = b.sessions.len().clamp(1, 8);
        Ok((Some(SectionsAllGlobal { sessions: b.sessions.clone(), projected, next: AtomicUsize::new(0) }), Some(threads)))
    }

    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> {
        Ok(Some(SectionsAllLocal { cur: None, off: 0 }))
    }

    fn exec(&self, _b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_sections_all: no global state"))?;
        let l = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_sections_all: no local state"))?;
        loop {
            if l.cur.is_none() {
                let i = g.next.fetch_add(1, Ordering::Relaxed);
                if i >= g.sessions.len() {
                    for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; }
                    return Ok(());
                }
                l.cur = Some(i);
                l.off = 0;
            }
            let idx = l.cur.unwrap();
            let (path, data) = &g.sessions[idx];
            let f = data.base()?;
            let n = f.sections.len();
            if l.off >= n { l.cur = None; continue; }
            let take = (n - l.off).min(out.capacity()?.max(1).min(1024));
            for (j, &orig) in g.projected.iter().enumerate() {
                match orig {
                    0 => {
                        let mut v = out.get_vector_at_mut::<String>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take { w.set(i, Some(path.as_str()))?; }
                    }
                    7 => {
                        let mut v = out.get_vector_at_mut::<bool>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take { w.set(i, Some(f.sections[l.off + i].in_list))?; }
                    }
                    _ => {
                        let mut v = out.get_vector_at_mut::<i64>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take {
                            let r = f.sections[l.off + i];
                            let val: Option<i64> = match orig {
                                1 => Some(r.section as i64),
                                2 => r.parent_section.map(|x| x as i64),
                                3 => Some(r.level as i64),
                                4 => Some(r.start as i64),
                                5 => Some(r.end as i64),
                                6 => Some(r.depth as i64),
                                _ => Some(r.header_node as i64),
                            };
                            w.set(i, val)?;
                        }
                    }
                }
            }
            l.off += take;
            return Ok(());
        }
    }
}

// --- complete parallel blocks scan over all sessions ---
struct BlocksBind { sessions: Arc<Vec<(String, Arc<SessionData>)>> }
struct BlocksGlobal { sessions: Arc<Vec<(String, Arc<SessionData>)>>, projected: Vec<usize>, next: AtomicUsize }
struct BlocksLocal { cur: Option<usize>, rows: Vec<BlockRow>, off: usize }
struct MdioBlocksAll;

impl TableFunctionCallbacks for MdioBlocksAll {
    type BindData = BlocksBind;
    type GlobalState = BlocksGlobal;
    type LocalState = BlocksLocal;

    fn bind(&self, context: &Context, arguments: Vec<BindArgument>, h: BindFunctionHandle<'_>) -> duckdb_neo::Result<Self::BindData> {
        let raw = arguments.first().and_then(|a| a.value.as_ref())
            .and_then(|v| v.get::<String>().ok().flatten())
            .ok_or_else(|| duckdb_neo::Error::api_error("mdio_blocks_all: sessions JSON must be a constant VARCHAR"))?;
        let specs: Vec<SessionSpec> = serde_json::from_str(&raw)
            .map_err(|e| duckdb_neo::Error::api_error(format!("mdio_blocks_all: bad sessions JSON: {e}")))?;
        let mut sessions = Vec::with_capacity(specs.len());
        for s in specs { sessions.push((s.path, session_data(s.sid as i64)?)); }
        h.set_order(OrderPreservation::None)?;
        h.add_result_column("path", &String::logical_type(context)?)?;
        h.add_result_column("node", &i64::logical_type(context)?)?;
        h.add_result_column("section", &i64::logical_type(context)?)?;
        h.add_result_column("type", &String::logical_type(context)?)?;
        h.add_result_column("heading", &String::logical_type(context)?)?;
        h.add_result_column("tag", &String::logical_type(context)?)?;
        for name in ["parent", "pos", "start_line", "end_line"] {
            h.add_result_column(name, &i64::logical_type(context)?)?;
        }
        h.add_result_column("text", &String::logical_type(context)?)?;
        Ok(BlocksBind { sessions: Arc::new(sessions) })
    }

    fn init_global_state(&self, b: &Self::BindData, _c: &Context, columns: InitColumnData<'_>) -> duckdb_neo::Result<(Option<Self::GlobalState>, Option<usize>)> {
        let projected = (0..columns.count()?).map(|i| columns.get_column_index(i)).collect::<duckdb_neo::Result<Vec<_>>>()?;
        let threads = b.sessions.len().clamp(1, 8);
        Ok((Some(BlocksGlobal { sessions: b.sessions.clone(), projected, next: AtomicUsize::new(0) }), Some(threads)))
    }

    fn init_local_state(&self, _b: &Self::BindData, _g: Option<&Self::GlobalState>, _c: &Context, _d: InitColumnData<'_>) -> duckdb_neo::Result<Option<Self::LocalState>> {
        Ok(Some(BlocksLocal { cur: None, rows: Vec::new(), off: 0 }))
    }

    fn exec(&self, _b: &Self::BindData, g: Option<&Self::GlobalState>, l: Option<&mut Self::LocalState>, _c: &Context, mut out: DataChunkRef<'_>, _d: ExecColumnInfo<'_>) -> duckdb_neo::Result<()> {
        let g = g.ok_or_else(|| duckdb_neo::Error::api_error("mdio_blocks_all: no global state"))?;
        let l = l.ok_or_else(|| duckdb_neo::Error::api_error("mdio_blocks_all: no local state"))?;
        loop {
            if l.off >= l.rows.len() {
                let i = g.next.fetch_add(1, Ordering::Relaxed);
                if i >= g.sessions.len() {
                    for j in 0..g.projected.len() { out.get_vector_at_mut::<Unknown>(j)?.set_size(0)?; }
                    return Ok(());
                }
                l.rows = build_blocks(&g.sessions[i].1)?;
                l.off = 0;
                l.cur = Some(i);
                if l.rows.is_empty() { continue; }
            }
            let take = (l.rows.len() - l.off).min(out.capacity()?.max(1).min(1024));
            let path = &g.sessions[l.cur.unwrap()].0;
            for (j, &orig) in g.projected.iter().enumerate() {
                match orig {
                    0 => {
                        let mut v = out.get_vector_at_mut::<String>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take { w.set(i, Some(path.as_str()))?; }
                    }
                    3 | 4 | 5 | 10 => {
                        let mut v = out.get_vector_at_mut::<String>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take {
                            let r = &l.rows[l.off + i];
                            let s: &str = match orig { 3 => &r.type_name, 4 => &r.heading, 5 => &r.tag, _ => &r.text };
                            w.set(i, Some(s))?;
                        }
                    }
                    _ => {
                        let mut v = out.get_vector_at_mut::<i64>(j)?;
                        v.set_size(take)?;
                        let mut w = v.writer()?;
                        for i in 0..take {
                            let r = &l.rows[l.off + i];
                            let val: Option<i64> = match orig {
                                1 => Some(r.node),
                                2 => Some(r.section),
                                _ => match orig {
                                    6 => r.parent,
                                    7 => r.pos,
                                    8 => r.start_line,
                                    _ => r.end_line,
                                },
                            };
                            w.set(i, val)?;
                        }
                    }
                }
            }
            l.off += take;
            return Ok(());
        }
    }
}

#[duckdb_entrypoint_c_api_v2(extension_name = "mdio_frame", api_version = "v2.0.0")]
pub fn extension_entrypoint(extension: &Extension) -> std::result::Result<(), Box<dyn std::error::Error>> {
    ScalarFunctionBuilder::new(
        "mdio_open",
        SignatureBuilder::new()
            .param(FunctionParameter::new::<String>("text"))
            .return_type(i64::logical_type(extension)?),
        OpenDoc,
    ).register(extension)?;
    ScalarFunctionBuilder::new(
        "mdio_open_snapshot",
        SignatureBuilder::new()
            .param(FunctionParameter::new::<duckdb_neo::types::BlobValue>("blob"))
            .return_type(i64::logical_type(extension)?),
        OpenSnapshot,
    ).register(extension)?;
    ScalarFunctionBuilder::new(
        "mdio_close",
        SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")).return_type(bool::logical_type(extension)?),
        CloseDoc,
    ).register(extension)?;
    ScalarFunctionBuilder::new(
        "mdio_render_all",
        SignatureBuilder::new()
            .param(FunctionParameter::new::<i64>("session"))
            .return_type(String::logical_type(extension)?),
        RenderAll,
    ).register(extension)?;
    ScalarFunctionBuilder::new(
        "mdio_render_selected",
        SignatureBuilder::new()
            .param(FunctionParameter::new::<i64>("session"))
            .param(FunctionParameter::new::<String>("nodes_json"))
            .return_type(String::logical_type(extension)?),
        RenderSelected,
    ).register(extension)?;
    TableFunctionBuilder::new("mdio_nodes", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), MdioNodes)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_faces", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), MdioFaces)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_faces_all", SignatureBuilder::new().param(FunctionParameter::new::<String>("sessions_json")), MdioFacesAll)
        .register(extension)?;
    TableFunctionBuilder::new("mdio_blocks_all", SignatureBuilder::new().param(FunctionParameter::new::<String>("sessions_json")), MdioBlocksAll)
        .register(extension)?;
    TableFunctionBuilder::new("mdio_sections_all", SignatureBuilder::new().param(FunctionParameter::new::<String>("sessions_json")), SectionsAll)
        .register(extension)?;
    TableFunctionBuilder::new("mdio_edges", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), Edges)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_roots", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), Roots)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_descendants", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), Descendants)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_sections", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), Sections)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_section_member", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), SectionMembers)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_span", SignatureBuilder::new().param(FunctionParameter::new::<i64>("session")), Spans)
        .with_projection_pushdown(true).register(extension)?;
    TableFunctionBuilder::new("mdio_meta", SignatureBuilder::new(), Meta)
        .with_projection_pushdown(true).register(extension)?;
    Ok(())
}
