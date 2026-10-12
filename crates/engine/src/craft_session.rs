use super::*;
use crate::craft_codec as codec;
use serde_json::json;
impl Session {
    pub fn craft_signature(&self, ui: &Value) -> String {
        json!([
            ui,
            self.catalog.revision,
            self.commits,
            self.undo.len(),
            self.redo.len(),
            self.selection,
            self.source,
            self.filter,
            self.sort,
            self.active_mask,
            self.active_spot,
            self.auto_sync,
            self.clipboard,
            self.browse,
            self.library_folder
        ])
        .to_string()
    }
    pub fn craft_capture(&self, ui: Value) -> std::result::Result<Vec<u8>, String> {
        let mut blobs = Vec::new();
        codec::push(&mut blobs, self.catalog.to_snapshot().into_bytes())?;
        codec::pack(
            &json!({"app":"lightcraft","schema":1,"ui":ui,"undo":self.undo,"redo":self.redo,"commits":self.commits,"selection":self.selection,"source":self.source,"filter":self.filter,"sort":self.sort,"mask":self.active_mask,"spot":self.active_spot,"autoSync":self.auto_sync,"clipboard":self.clipboard,"browse":self.browse,"folder":self.library_folder}),
            &blobs,
        )
    }
    pub fn craft_restore(&mut self, bytes: &[u8]) -> std::result::Result<Value, String> {
        let (v, blobs) = codec::unpack(bytes)?;
        if v["app"] != "lightcraft" || v["schema"] != 1 {
            return Err("Incompatible LightCraft session".into());
        }
        let cat = Catalog::from_snapshot(std::str::from_utf8(codec::blob(&blobs, 0)?).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let undo = codec::decode(&v["undo"])?;
        let redo = codec::decode(&v["redo"])?;
        let selection = codec::decode(&v["selection"])?;
        let source = codec::decode(&v["source"])?;
        let filter = codec::decode(&v["filter"])?;
        let sort = codec::decode(&v["sort"])?;
        let mask = codec::decode(&v["mask"])?;
        let spot = codec::decode(&v["spot"])?;
        let sync = codec::decode(&v["autoSync"])?;
        let clipboard = codec::decode(&v["clipboard"])?;
        let browse = codec::decode(&v["browse"])?;
        let folder = codec::decode(&v["folder"])?;
        let commits = codec::decode(&v["commits"])?;
        self.catalog = cat;
        self.undo = undo;
        self.redo = redo;
        self.selection = selection;
        self.source = source;
        self.filter = filter;
        self.sort = sort;
        self.active_mask = mask;
        self.active_spot = spot;
        self.auto_sync = sync;
        self.clipboard = clipboard;
        self.browse = browse;
        self.library_folder = folder;
        self.commits = commits;
        self.visible_key = None;
        self.total = None;
        self.interaction = None;
        if v["app"] != "lightcraft" || v["schema"] != 1 {
            return Err("Incompatible LightCraft session".into());
        }
        Ok(v["ui"].clone())
    }
}

impl Session {
    pub fn craft_ui(bytes: &[u8]) -> std::result::Result<Value, String> {
        let (v, _) = crate::craft_codec::unpack(bytes)?;
        if v["app"] != "lightcraft" || v["schema"] != 1 {
            return Err("Incompatible LightCraft session".into());
        }
        Ok(v["ui"].clone())
    }
}
