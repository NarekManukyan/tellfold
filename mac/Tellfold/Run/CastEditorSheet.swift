import SwiftUI

/// Opens when a "Review the cast before the run" simulation has written its people and waits. Lists everyone with
/// their role, camp and starting stance; the user can remove people, change a stance or bio, add a group described in
/// words (the engine writes them), save the cast as an audience, and then start the run. Nothing is simulated yet,
/// so fixing the cast here costs nothing but the group calls.
struct CastEditorSheet: View {
    @Bindable var store: RunStore
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss

    @State private var model: CastEditorModel
    @State private var groupText = ""
    @State private var groupCount = 3
    @State private var savingAudience = false
    @State private var audienceName = ""
    @State private var savedNote: String?
    @State private var confirmStop = false

    init(store: RunStore) {
        self.store = store
        _model = State(initialValue: CastEditorModel(runID: store.runID, api: store.api))
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Seam()
            list
            Seam()
            groupBar
            Seam()
            footer
        }
        .background(Theme.ground)
        .frame(minWidth: 640, idealWidth: 720, minHeight: 560, idealHeight: 720)
        .task {
            await model.load()
            await store.reloadDetail() // the topic map, for "Save as Audience"
        }
        .onChange(of: store.showsCastEditor) { _, shown in if !shown { dismiss() } }
        .alert("Save as Audience", isPresented: $savingAudience) {
            TextField("Name", text: $audienceName)
            Button("Save") { saveAudience() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Saved audiences can start a new simulation without casting it again.")
        }
        .confirmationDialog("Stop this simulation?", isPresented: $confirmStop) {
            Button("Stop Simulation", role: .destructive) { Task { await store.stop() } }
        } message: {
            Text("Nothing has been simulated yet, so no report is written.")
        }
    }

    // MARK: header

    private var header: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            Text("Check the cast")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            Text(store.question)
                .font(Theme.body)
                .foregroundStyle(Theme.secondary)
                .lineLimit(2)
            Text(summary)
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            if !model.campCounts.isEmpty {
                HStack(spacing: 6) {
                    ForEach(model.campCounts, id: \.camp) { item in
                        Text("\(item.camp.title) \(item.count)")
                            .font(Theme.small)
                            .foregroundStyle(Theme.secondary)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(Theme.hover, in: Capsule())
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, Theme.space5)
        .padding(.top, Theme.space5)
        .padding(.bottom, Theme.space3)
    }

    private var summary: String {
        let n = model.people.count
        let people = "\(n) \(n == 1 ? "person is" : "people are") ready."
        return "\(people) Remove anyone who does not belong, change what someone thinks, or add a group. "
            + "Nothing has been simulated yet."
    }

    // MARK: list

    @ViewBuilder
    private var list: some View {
        if model.isLoading && model.people.isEmpty {
            ProgressView("Loading the cast…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if model.people.isEmpty {
            ContentUnavailableView("No One Left", systemImage: "person.2.slash",
                                   description: Text("Add a group below, or stop the simulation."))
        } else {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(model.people) { person in
                        CastRow(person: person, isEditing: model.editingID == person.id,
                                edit: { model.toggleEditing(person.id) },
                                remove: { model.remove(person.id) },
                                save: { edited, original in model.update(edited, from: original) })
                        if person.id != model.people.last?.id { Seam().padding(.leading, 60) }
                    }
                }
                .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
                .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius).strokeBorder(Theme.seam, lineWidth: 1))
                .padding(.horizontal, Theme.space5)
                .padding(.vertical, Theme.space3)
            }
        }
    }

    // MARK: add a group

    private var groupBar: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            Text("Add a group").font(Theme.sectionTitle).foregroundStyle(Theme.text)
            HStack(spacing: Theme.space2) {
                TextField("Group", text: $groupText,
                          prompt: Text("e.g. parents of Year 7 children who take the bus"))
                    .labelsHidden()
                    .consoleField()
                    .onSubmit { Task { await addGroup() } }
                Stepper(value: $groupCount, in: 1...CastEditorModel.maxGroup) {
                    Text("\(groupCount) \(groupCount == 1 ? "person" : "people")")
                        .font(Theme.mono(12))
                        .foregroundStyle(Theme.secondary)
                        .frame(width: 72, alignment: .trailing)
                }
                .fixedSize()
                .accessibilityLabel("How many people")
                .accessibilityValue("\(groupCount)")
                Button {
                    Task { await addGroup() }
                } label: {
                    if model.isAddingGroup {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("Add")
                    }
                }
                .accessibilityLabel(model.isAddingGroup ? "Adding the group" : "Add")
                .buttonStyle(.console(.secondary, compact: true))
                .disabled(groupText.trimmingCharacters(in: .whitespaces).isEmpty || model.isBusy
                          || model.people.count >= CastEditorModel.maxPeople)
                .help("The planner model writes these people from your description (one model call)")
            }
        }
        .padding(.horizontal, Theme.space5)
        .padding(.vertical, Theme.space3)
    }

    private func addGroup() async {
        let count = min(groupCount, CastEditorModel.maxPeople - model.people.count)
        guard count > 0 else { return }
        if await model.addGroup(description: groupText, count: count) { groupText = "" }
    }

    // MARK: footer

    private var footer: some View {
        HStack(alignment: .center, spacing: Theme.space3) {
            VStack(alignment: .leading, spacing: 2) {
                if let error = model.errorMessage {
                    Text(error).font(Theme.small).foregroundStyle(Theme.negative)
                        .fixedSize(horizontal: false, vertical: true)
                } else if let savedNote {
                    Text(savedNote).font(Theme.small).foregroundStyle(Theme.muted)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Save as Audience…") {
                audienceName = AudienceStore.cleanName(store.question.isEmpty ? "" : String(store.question.prefix(60)))
                savingAudience = true
            }
            .buttonStyle(.consoleGhost)
            .disabled(model.people.isEmpty || model.isBusy)
            Button("Stop", role: .destructive) { confirmStop = true }
                .buttonStyle(.consoleGhost)
                .disabled(model.isContinuing)
            Button("Later") {
                // the edits go to the waiting run first, so they are there when the editor opens again
                Task {
                    if await model.saveForLater() { store.showsCastEditor = false; dismiss() }
                }
            }
            .buttonStyle(.console)
            .keyboardShortcut(.cancelAction)
            .disabled(model.isBusy)
            .help("Close this; the simulation keeps waiting. Check the Cast in the toolbar opens it again.")
            Button {
                Task {
                    if await model.confirm() { store.showsCastEditor = false; dismiss() }
                }
            } label: {
                if model.isContinuing {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Start Simulation")
                }
            }
            .accessibilityLabel(model.isContinuing ? "Starting the simulation" : "Start Simulation")
            .buttonStyle(.consolePrimary)
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(!model.canContinue)
            .help("Run the simulation with these \(model.people.count) people (⌘↩)")
        }
        .padding(.horizontal, Theme.space5)
        .padding(.vertical, Theme.space4)
    }

    private func saveAudience() {
        guard let saved = model.saveAudience(name: audienceName, question: store.question, graph: store.graph,
                                             to: app.audiences) else { return }
        savedNote = "Saved as \u{201C}\(saved.name)\u{201D}."
    }
}

/// One person in the cast editor: avatar, name and handle, role, camp and starting stance, with Edit and Remove.
/// Editing opens the bio, stance and camp in place.
private struct CastRow: View {
    let person: Persona
    let isEditing: Bool
    let edit: () -> Void
    let remove: () -> Void
    /// The edited person and the person the edit started from.
    let save: (Persona, Persona) -> Void

    /// The edit, and the person it started from. A row is reused for whoever has its id next (the engine renumbers),
    /// so both are taken again whenever the row's person changes.
    @State private var draft: Persona
    @State private var original: Persona

    init(person: Persona, isEditing: Bool, edit: @escaping () -> Void, remove: @escaping () -> Void,
         save: @escaping (Persona, Persona) -> Void) {
        self.person = person
        self.isEditing = isEditing
        self.edit = edit
        self.remove = remove
        self.save = save
        _draft = State(initialValue: person)
        _original = State(initialValue: person)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            HStack(alignment: .top, spacing: Theme.space3) {
                AvatarView(persona: person, size: 32)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(person.name).font(Theme.bodyStrong).foregroundStyle(Theme.text).lineLimit(1)
                        Text("@\(person.handle)").font(Theme.mono(11)).foregroundStyle(Theme.muted).lineLimit(1)
                        switch PersonaSource(person) {
                        case .group: Text("Added").font(Theme.caption).foregroundStyle(Theme.accent)
                        case .edited: Text("Edited").font(Theme.caption).foregroundStyle(Theme.accent)
                        case .cast, nil: EmptyView()
                        }
                    }
                    Text(role).font(Theme.small).foregroundStyle(Theme.secondary).lineLimit(2)
                    if !isEditing {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(CastCamp.label(for: person))
                                .font(Theme.small)
                                .foregroundStyle(Theme.text)
                                .padding(.horizontal, 7)
                                .padding(.vertical, 2)
                                .background(Theme.hover, in: Capsule())
                            Text(person.stance)
                                .font(Theme.small)
                                .foregroundStyle(Theme.muted)
                                .lineLimit(2)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Button(action: edit) {
                    Image(systemName: isEditing ? "chevron.up" : "pencil")
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.muted)
                .help(isEditing ? "Close without saving" : "Edit \(person.name)")
                .accessibilityLabel(isEditing ? "Close editor" : "Edit \(person.name)")
                Button(action: remove) {
                    Image(systemName: "trash")
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.muted)
                .help("Remove \(person.name) from the cast")
                .accessibilityLabel("Remove \(person.name)")
            }
            if isEditing { editor.padding(.leading, 44) }
        }
        .padding(.horizontal, Theme.space3)
        .padding(.vertical, 10)
        .onChange(of: isEditing) { _, open in if open { start() } }
        .onChange(of: person) { _, _ in start() }
        .accessibilityElement(children: .contain)
    }

    private func start() {
        draft = person
        original = person
    }

    /// The bio is the person's public role line; organisations and public figures are marked as such.
    private var role: String {
        let kind = person.entityID != nil ? "Organisation or public figure" : "Member of the public"
        return person.bio.isEmpty ? kind : "\(kind) · \(person.bio)"
    }

    private var editor: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            field("Public bio") {
                TextField("Public bio", text: $draft.bio, axis: .vertical).labelsHidden().consoleField()
            }
            field("Starting stance (private)") {
                TextField("Starting stance", text: $draft.stance, axis: .vertical)
                    .lineLimit(2...5)
                    .labelsHidden()
                    .consoleField()
            }
            field("Camp") {
                Picker("Camp", selection: $draft.camp) {
                    if draft.camp.flatMap(CastCamp.init(rawValue:)) == nil {
                        Text(CastCamp.label(for: person)).tag(draft.camp)
                    }
                    ForEach(CastCamp.allCases) { Text($0.title).tag(Optional($0.rawValue)) }
                }
                .labelsHidden()
                .fixedSize()
            }
            HStack {
                Spacer()
                Button("Save") { save(draft, original) }
                    .buttonStyle(.console(.primary, compact: true))
                    .disabled(draft.stance.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    private func field<Content: View>(_ title: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(Theme.caption).foregroundStyle(Theme.muted)
            content()
        }
    }
}
