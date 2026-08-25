#[allow(
    clippy::wildcard_imports,
    reason = "preview implementation modules share the renderer boundary namespace"
)]
use super::*;

impl PreviewRenderer {
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = builtin_plugin();
        runtime.register_plugin(plugin.as_ref())?;

        let mut renderer = Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            revision,
            applied: empty_project(project),
            source_ids: HashMap::new(),
            filter_diagnostics: Vec::new(),
            scene_ids: HashSet::new(),
            static_scenes: HashSet::new(),
            static_frames: HashMap::new(),
            static_preview_frames: HashMap::new(),
            scene_layer_cache: VecDeque::new(),
            compositor: PreviewCompositor::new(format),
            gpu_program_scene: None,
            preview_scaler: None,
            applied_draft: None,
        };
        // Building from an empty mirror of the same profile makes the first
        // build and every later update the same code path, so there is exactly
        // one description of how a project becomes runtime state.
        renderer.apply_profile(project)?;
        renderer.applied = project.clone();
        Ok(renderer)
    }

    /// Brings the runtime in line with `project` without recreating sources.
    ///
    /// Returns whether anything was applied, so the caller can skip the UI work
    /// that depends on project content.
    ///
    /// Moving, hiding, reordering, renaming, or filtering a source is a scene
    /// graph edit. Only a changed canvas, a changed profile, or changed source
    /// settings can reach the capture devices, and even then only the sources
    /// that actually changed.
    pub(crate) fn sync_project(
        &mut self,
        project: &Project,
        revision: u64,
    ) -> Result<bool, Box<dyn Error>> {
        if revision == self.revision {
            return Ok(false);
        }
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        // A different canvas means every source renegotiates its output shape.
        // Profile changes and source-kind changes are diffed below so sources
        // shared by two profiles can stay open while the scene graph moves.
        let rebuild = profile.video_format() != self.format;
        if rebuild {
            *self = Self::new(project, revision)?;
            return Ok(true);
        }
        // A project revision may change source content or scene metadata while
        // the media timestamp is still the same. Do not let a snapshot from
        // the previous graph cross that revision boundary.
        self.scene_layer_cache.clear();
        self.apply_profile(project)?;
        self.applied = project.clone();
        self.revision = revision;
        // The draft is expressed against project state that has just moved, so
        // it is re-applied from scratch on the next render.
        self.applied_draft = None;
        Ok(true)
    }

    /// Returns whether a live source changed its kind and therefore needs its
    /// scene references detached before the old factory instance is replaced.
    fn kind_changed(&self, profile: &Profile) -> bool {
        let Some(applied) = self.applied.active_profile_spec() else {
            return true;
        };
        profile.sources().any(|source| {
            applied
                .source(source.id())
                .is_some_and(|previous| previous.kind() != source.kind())
        })
    }

    /// Applies the difference between the mirrored project and `project`.
    fn apply_profile(&mut self, project: &Project) -> Result<(), Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        self.filter_diagnostics = Self::collect_filter_diagnostics(profile);
        // The mirrored profile is cloned out first: `sync_source` needs `&mut
        // self`, and the borrow checker will not hold a reference into
        // `self.applied` across it.
        let previous = self
            .applied
            .active_profile_spec()
            .map(|profile| {
                profile
                    .sources()
                    .map(|source| (source.id().as_str().to_owned(), source.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        if self.kind_changed(profile) {
            for scene in self.scene_ids.clone() {
                self.runtime.clear_scene_sources(&scene)?;
            }
        }

        let active_sources = Self::active_source_ids(profile)?;
        for source in profile
            .sources()
            .filter(|source| active_sources.contains(source.id().as_str()))
        {
            self.sync_source(source, previous.get(source.id().as_str()))?;
        }
        self.sync_scenes(profile)?;
        self.retire_sources(profile)?;

        self.static_scenes = static_scenes(profile);
        // Cached still frames describe scene content that may have just moved.
        self.static_frames.clear();
        self.static_preview_frames.clear();
        // The full program target is retained by the compositor between GUI
        // requests. Any project diff invalidates that GPU-side snapshot too;
        // otherwise an output request for the same scene ID could reuse pixels
        // from before the edit.
        self.gpu_program_scene = None;
        Ok(())
    }

    /// Creates or updates one source without disturbing the others.
    fn sync_source(
        &mut self,
        source: &SourceSpec,
        previous: Option<&SourceSpec>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(&id) = self.source_ids.get(source.id().as_str()) else {
            let id = self.runtime.create_source(
                source.kind().as_str(),
                source.name(),
                source.settings(),
            )?;
            self.apply_filters(id, source)?;
            self.source_ids.insert(source.id().as_str().to_owned(), id);
            return Ok(());
        };
        let previous = previous.ok_or_else(|| {
            std::io::Error::other(format!("source {} has no mirrored state", source.id()))
        })?;
        if previous.kind() != source.kind() {
            // Runtime source instances cannot change factory kind in place.
            // Scene references were cleared by `apply_profile` before this
            // path, so destroying this one source does not disturb any other
            // capture device.
            self.runtime.destroy_source(id)?;
            let id = self.runtime.create_source(
                source.kind().as_str(),
                source.name(),
                source.settings(),
            )?;
            self.apply_filters(id, source)?;
            self.source_ids.insert(source.id().as_str().to_owned(), id);
            return Ok(());
        }
        // Only settings can reach the device, so only settings restart it.
        if previous.settings() != source.settings() {
            self.runtime.update_source(id, source.settings())?;
        }
        if previous.name() != source.name() {
            self.runtime.rename_source(id, source.name())?;
        }
        if previous.filters() != source.filters() {
            self.runtime.clear_source_filters(id)?;
            self.apply_filters(id, source)?;
        }
        Ok(())
    }

    fn apply_filters(&mut self, id: SourceId, source: &SourceSpec) -> Result<(), Box<dyn Error>> {
        for filter in source.filters() {
            if let FilterCompilation::Applied(runtime_filter) = compile_filter_report(filter) {
                self.runtime.add_source_filter(id, runtime_filter)?;
            }
        }
        Ok(())
    }

    /// Collects unavailable persisted filters without adding them to the
    /// renderer. The list is capped so a malformed project cannot inflate
    /// every diagnostics snapshot.
    fn collect_filter_diagnostics(profile: &Profile) -> Vec<String> {
        let mut diagnostics = Vec::new();
        for source in profile.sources() {
            for filter in source.filters() {
                let FilterCompilation::Unavailable(diagnostic) = compile_filter_report(filter)
                else {
                    continue;
                };
                if diagnostics.len() + 1 < MAX_FILTER_DIAGNOSTICS {
                    diagnostics.push(format!(
                        "source '{}' filter '{}': {diagnostic}",
                        source.name(),
                        filter.name()
                    ));
                } else if diagnostics.len() + 1 == MAX_FILTER_DIAGNOSTICS {
                    diagnostics.push(format!(
                        "additional filter diagnostics omitted after {MAX_FILTER_DIAGNOSTICS} entries"
                    ));
                }
            }
        }
        diagnostics
    }

    /// Rebuilds every scene's composition order in place.
    fn sync_scenes(&mut self, profile: &Profile) -> Result<(), Box<dyn Error>> {
        let live = profile
            .scenes()
            .map(|scene| scene.id().as_str().to_owned())
            .collect::<HashSet<_>>();
        for scene in self.scene_ids.clone() {
            if !live.contains(&scene) {
                self.runtime.destroy_scene(&scene)?;
                self.scene_ids.remove(&scene);
            }
        }

        for scene in profile.scenes() {
            let name = scene.id().as_str();
            if self.scene_ids.insert(name.to_owned()) {
                self.runtime.create_scene(name)?;
            }
            let flattened = self.visible_items(profile, name)?;
            let order = flattened
                .iter()
                .map(|(_, source, _)| *source)
                .collect::<Vec<_>>();
            let item_ids = flattened
                .iter()
                .map(|(item_id, _, _)| item_id.clone())
                .collect::<Vec<_>>();
            let attached = self
                .runtime
                .scene_sources(name)
                .map(<[SourceId]>::to_vec)
                .unwrap_or_default();
            let attached_item_ids = self.runtime.scene_item_ids(name).unwrap_or_default();
            if attached != order || attached_item_ids != item_ids {
                // Rebuild only the scene-item references. The shared runtime
                // source instances stay alive, so changing visibility/order or
                // adding a second reference never reopens a capture device.
                self.runtime.clear_scene_sources(name)?;
                for (item_id, source, _) in &flattened {
                    self.runtime
                        .attach_source_instance_with_id(name, *source, item_id)?;
                }
            }
            for (item_id, _, transform) in &flattened {
                self.runtime
                    .set_scene_item_transform_by_id(name, item_id, *transform)?;
            }
        }
        Ok(())
    }

    /// Resolves a scene's visible items, including nested scene references, to
    /// runtime sources and composed transforms in draw order.
    ///
    /// Keeps every visible scene item in draw order, including repeated
    /// references to one shared runtime source.
    fn visible_items(
        &self,
        profile: &Profile,
        scene_id: &str,
    ) -> Result<Vec<VisibleItem>, Box<dyn Error>> {
        profile
            .flatten_scene_items(scene_id)?
            .into_iter()
            .map(|item| {
                let source = self
                    .source_ids
                    .get(item.source_id().as_str())
                    .copied()
                    .ok_or_else(|| {
                        std::io::Error::other(format!(
                            "scene item references unknown source {}",
                            item.source_id()
                        ))
                    })?;
                Ok((item.item_id().to_owned(), source, item.transform()))
            })
            .collect()
    }

    /// Resolves the source instances that have at least one visible scene
    /// consumer. A source definition can exist in the project without being
    /// live in the preview runtime; keeping that distinction prevents hidden
    /// cameras and screen-cast sessions from claiming hardware.
    fn active_source_ids(profile: &Profile) -> Result<HashSet<String>, Box<dyn Error>> {
        let mut active = HashSet::new();
        for scene in profile.scenes() {
            for item in profile.flatten_scene_items(scene.id().as_str())? {
                active.insert(item.source_id().as_str().to_owned());
            }
        }
        Ok(active)
    }

    /// Destroys sources the project no longer defines.
    ///
    /// A hidden source is detached from every scene and therefore produces no
    /// composition work, but its runtime identity stays warm. Reusing that
    /// identity avoids reopening a camera or screen capture when the operator
    /// makes the source visible again.
    fn retire_sources(&mut self, profile: &Profile) -> Result<(), Box<dyn Error>> {
        let removed = self
            .source_ids
            .iter()
            .filter(|(id, _)| !profile.has_source(id.as_str()))
            .map(|(id, source)| (id.clone(), *source))
            .collect::<Vec<_>>();
        for (project_id, source) in removed {
            self.runtime.destroy_source(source)?;
            self.source_ids.remove(&project_id);
        }
        Ok(())
    }

    /// Takes backend-generated settings that became available after an
    /// asynchronous source open, such as a fresh Wayland restore token.
    pub(crate) fn take_source_settings_updates(&mut self) -> Vec<(String, String, Config)> {
        let profile = self.applied.active_profile().as_str().to_owned();
        let sources = self
            .source_ids
            .iter()
            .map(|(project_id, source_id)| (project_id.clone(), *source_id))
            .collect::<Vec<_>>();
        sources
            .into_iter()
            .filter_map(|(project_id, source_id)| {
                self.runtime
                    .take_source_settings_update(source_id)
                    .map(|settings| (profile.clone(), project_id, settings))
            })
            .collect()
    }

    /// Applies, replaces, or withdraws the canvas drag's transform.
    ///
    /// The runtime holds the dragged transform only while the pointer is down;
    /// letting go restores whatever the project says, which is either the
    /// committed drag or the untouched original if the drag was abandoned.
    pub(crate) fn set_transform_draft(&mut self, draft: Option<&TransformDraft>) {
        let target = draft.and_then(|draft| {
            let profile = self.applied.active_profile_spec()?;
            let scene = profile.scene(draft.scene.as_str())?;
            // Group leaves are attached to the runtime with the same stable
            // path that the project flattener returns. Keep that lookup here
            // so a pointer draft updates the live nested layer instead of
            // being silently discarded by the root-item check below.
            let flattened = profile.flatten_scene_items(draft.scene.as_str()).ok()?;
            let mut targets = Vec::with_capacity(draft.items.len());
            for item in &draft.items {
                let is_visible = if let Some(scene_item) = scene.item(item.item.as_str()) {
                    if scene_item.is_scene_reference() || scene_item.is_group() {
                        return None;
                    }
                    scene_item.visible()
                } else {
                    flattened
                        .iter()
                        .any(|candidate| candidate.item_id() == item.item)
                };
                if !is_visible {
                    return None;
                }
                targets.push((item.item.clone(), item.transform));
            }
            Some((draft.scene.clone(), targets))
        });
        let same_targets = match (&self.applied_draft, &target) {
            (Some((scene, sources)), Some((next_scene, next))) => {
                next_scene == scene
                    && next
                        .iter()
                        .map(|(item_id, _)| item_id)
                        .eq(sources.iter().map(|(item_id, _)| item_id))
            }
            _ => false,
        };
        if !same_targets {
            if let Some((scene, sources)) = self.applied_draft.take() {
                for (item_id, committed) in sources {
                    let _ = self
                        .runtime
                        .set_scene_item_transform_by_id(&scene, &item_id, committed);
                }
                // A scene composed only of still sources caches its picture, so
                // the cache has to go when the drag stops moving it.
                self.invalidate_static_scene_cache(&scene);
            }
        }
        let (Some(_draft), Some((scene, targets))) = (draft, target) else {
            return;
        };
        let committed = if same_targets {
            self.applied_draft
                .as_ref()
                .map(|(_, sources)| sources.clone())
                .unwrap_or_default()
        } else {
            let Some(committed) = targets
                .iter()
                .map(|(item_id, _)| {
                    self.runtime
                        .scene_item_transform_by_id(&scene, item_id)
                        .map(|transform| (item_id.clone(), transform))
                })
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            committed
        };
        for (item_id, transform) in targets {
            if self
                .runtime
                .set_scene_item_transform_by_id(&scene, &item_id, transform)
                .is_err()
            {
                for (committed_item, committed_transform) in committed {
                    let _ = self.runtime.set_scene_item_transform_by_id(
                        &scene,
                        &committed_item,
                        committed_transform,
                    );
                }
                self.invalidate_static_scene_cache(&scene);
                return;
            }
        }
        if !same_targets && !committed.is_empty() {
            self.invalidate_static_scene_cache(&scene);
            self.applied_draft = Some((scene, committed));
        }
    }

    /// Returns the engine snapshot the studio window shows.
    pub(crate) fn diagnostics(&self) -> RuntimeDiagnostics {
        RuntimeDiagnostics {
            metrics: self.runtime.compositor_metrics(),
            usage: self.runtime.usage(),
            limits: self.runtime.limits(),
            failures: self
                .runtime
                .source_failures()
                .into_iter()
                .map(|(source, failure)| {
                    let name = self
                        .runtime
                        .source_info(source)
                        .map_or_else(String::new, |(_, name)| name.to_owned());
                    format!("{name}: {failure}")
                })
                .collect(),
            filter_diagnostics: self.filter_diagnostics.clone(),
        }
    }
}
