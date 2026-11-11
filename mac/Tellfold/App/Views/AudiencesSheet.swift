import SwiftUI

/// The saved audiences, to rename or delete. Opened from "Manage…" next to the People picker.
struct AudiencesSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var renamingID: String?
    @State private var newName = ""
    @State private var deleting: SavedAudience?
    @State private var errorMessage: String?
    @FocusState private var nameFocused: Bool

    private var store: AudienceStore { model.audiences }

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: Theme.space1) {
                Text("Saved audiences")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text("Save a cast from the cast editor to start later simulations with the same people.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, Theme.space5)
            .padding(.top, Theme.space5)
            .padding(.bottom, Theme.space3)
            Seam()
            if store.audiences.isEmpty {
                ContentUnavailableView("No Saved Audiences", systemImage: "person.3",
                                       description: Text("Check the cast before a run, then choose Save as Audience."))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(store.audiences) { audience in
                        row(audience)
                    }
                }
                .listStyle(.inset)
            }
            Seam()
            HStack {
                if let errorMessage {
                    Text(errorMessage).font(Theme.small).foregroundStyle(Theme.negative)
                }
                Spacer()
                Button("Done") { dismiss() }
                    .buttonStyle(.consolePrimary)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, Theme.space5)
            .padding(.vertical, Theme.space4)
        }
        .background(Theme.ground)
        .frame(minWidth: 520, idealWidth: 560, minHeight: 360, idealHeight: 440)
        .onAppear { store.reload() }
        .confirmationDialog("Delete \u{201C}\(deleting?.name ?? "")\u{201D}?", isPresented: Binding(
            get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Move to Trash", role: .destructive) {
                if let id = deleting?.id { perform { try store.delete(id) } }
                deleting = nil
            }
        } message: {
            Text("Simulations already run with these people keep them.")
        }
    }

    private func row(_ audience: SavedAudience) -> some View {
        HStack(spacing: Theme.space3) {
            VStack(alignment: .leading, spacing: 2) {
                if renamingID == audience.id {
                    TextField("Name", text: $newName)
                        .labelsHidden()
                        .consoleField()
                        .focused($nameFocused)
                        .onSubmit { commitRename(audience.id) }
                        .onExitCommand { renamingID = nil }
                } else {
                    Text(audience.name).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                }
                Text(detail(audience)).font(Theme.small).foregroundStyle(Theme.muted).lineLimit(1)
            }
            Spacer()
            if renamingID == audience.id {
                Button("Save") { commitRename(audience.id) }
                    .buttonStyle(.console(.primary, compact: true))
            } else {
                Button("Rename") {
                    newName = audience.name
                    renamingID = audience.id
                    nameFocused = true
                }
                .buttonStyle(.console(.ghost, compact: true))
                Button {
                    deleting = audience
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.console(.ghost, compact: true))
                .help("Delete \(audience.name)")
                .accessibilityLabel("Delete \(audience.name)")
            }
        }
        .padding(.vertical, 4)
    }

    private func detail(_ audience: SavedAudience) -> String {
        let n = audience.personas.count
        let date = audience.createdAt.formatted(date: .abbreviated, time: .omitted)
        let people = "\(n) \(n == 1 ? "person" : "people") · saved \(date)"
        return audience.question.isEmpty ? people : "\(people) · \(audience.question)"
    }

    private func commitRename(_ id: String) {
        perform { try store.rename(id, to: newName) }
        renamingID = nil
    }

    private func perform(_ change: () throws -> Void) {
        do {
            try change()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
