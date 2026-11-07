import SwiftUI

/// The simulated feed, the Crowd tab's side panel when nothing is selected: posts grouped by round (the round at the
/// playhead by default), replies threaded under their parent, repost quotes, engagement counts, the amber event card
/// pinned at the top of its round, and handle links that select agents. The field at the top filters it.
struct FeedView: View {
    @Bindable var store: RunStore
    /// Filter text: post text or author handle.
    @State private var search = ""
    @State private var scope: Scope = .playheadRound

    enum Scope: String, CaseIterable, Identifiable {
        case throughPlayhead, playheadRound, all
        var id: String { rawValue }
        var title: String {
            switch self {
            case .throughPlayhead: "Up to this round"
            case .playheadRound: "This round"
            case .all: "All rounds"
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            filterBar
            Seam()
            content
        }
        .background(Theme.card)
    }

    // MARK: model

    struct Row: Identifiable {
        let post: PostOut
        let depth: Int
        /// Parent post when the reply's parent sits in an earlier round (shown as context line).
        let replyingTo: PostOut?
        let quoted: PostOut?
        var id: Int { post.id }
    }

    struct RoundGroup: Identifiable {
        let round: Int
        let events: [GodEvent]
        let rows: [Row]
        var id: Int { round }
    }

    private var postsByID: [Int: PostOut] {
        Dictionary(store.posts.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
    }

    private func visible(_ post: PostOut) -> Bool {
        switch scope {
        case .throughPlayhead: post.round <= store.playhead
        case .playheadRound: post.round == store.playhead
        case .all: true
        }
    }

    private func matchesSearch(_ post: PostOut) -> Bool {
        let q = search.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return true }
        return post.content.localizedCaseInsensitiveContains(q)
            || post.authorHandle.localizedCaseInsensitiveContains(q.trimmingCharacters(in: CharacterSet(charactersIn: "@")))
    }

    private var groups: [RoundGroup] {
        let byID = postsByID
        let shown = store.posts.filter { visible($0) && matchesSearch($0) }
        let byRound = Dictionary(grouping: shown, by: \.round)
        let events = Dictionary(grouping: store.detail?.events ?? [], by: \.round)
        var rounds = Set(byRound.keys)
        for r in events.keys where scope == .all || (scope == .throughPlayhead ? r <= store.playhead : r == store.playhead) {
            if search.isEmpty { rounds.insert(r) }
        }
        return rounds.sorted().map { round in
            let posts = (byRound[round] ?? []).sorted { $0.id < $1.id }
            return RoundGroup(round: round, events: events[round] ?? [], rows: threadRows(posts, byID: byID))
        }
    }

    /// Depth-first threading inside one round: a reply whose parent is in the same round nests under it.
    private func threadRows(_ posts: [PostOut], byID: [Int: PostOut]) -> [Row] {
        let ids = Set(posts.map(\.id))
        var children: [Int: [PostOut]] = [:]
        var roots: [PostOut] = []
        for post in posts {
            if let parent = post.parentID, ids.contains(parent) {
                children[parent, default: []].append(post)
            } else {
                roots.append(post)
            }
        }
        var rows: [Row] = []
        func visit(_ post: PostOut, depth: Int) {
            let parent = post.parentID.flatMap { byID[$0] }
            rows.append(Row(
                post: post,
                depth: depth,
                replyingTo: depth == 0 ? parent : nil,
                quoted: post.repostOf.flatMap { byID[$0] }
            ))
            for child in children[post.id] ?? [] { visit(child, depth: min(depth + 1, 4)) }
        }
        roots.forEach { visit($0, depth: 0) }
        return rows
    }

    // MARK: views

    private var filterBar: some View {
        VStack(spacing: Theme.space2) {
            HStack(spacing: Theme.space2) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(store.playhead == 0 ? "Opening posts" : "Round \(store.playhead)")
                        .font(Theme.sectionTitle)
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .accessibilityAddTraits(.isHeader)
                    let count = store.posts.lazy.filter { visible($0) && matchesSearch($0) }.count
                    HStack(spacing: 5) {
                        Text("\(count) \(count == 1 ? "post" : "posts")")
                            .font(Theme.small)
                            .foregroundStyle(Theme.muted)
                        if scope == .playheadRound, let mean = RunUI.roundSentiment(store.playhead, in: store) {
                            Text("mean").font(Theme.small).foregroundStyle(Theme.muted)
                            SentimentValue(score: mean, size: 12)
                        }
                    }
                    .lineLimit(1)
                }
                .fixedSize()
                Spacer(minLength: Theme.space2)
                Picker("Show", selection: $scope) {
                    ForEach(Scope.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.menu)
                .controlSize(.small)
                .fixedSize()
                .labelsHidden()
                .accessibilityLabel("Rounds shown in the feed")
            }
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(Theme.muted)
                TextField("Search posts and people", text: $search)
                    .textFieldStyle(.plain)
                    .font(Theme.body)
                    .accessibilityLabel("Search the feed")
                if !search.isEmpty {
                    Button { search = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.faint)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 28)
            .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
        }
        .padding(.horizontal, Theme.space4)
        .padding(.vertical, Theme.space3)
    }

    @ViewBuilder
    private var content: some View {
        if store.posts.isEmpty {
            if store.isLive {
                ContentUnavailableView {
                    Label("Waiting for Posts", systemImage: "text.bubble")
                } description: {
                    Text("Posts appear here once agents start talking.")
                } actions: {
                    ProgressView().controlSize(.small)
                }
            } else {
                ContentUnavailableView("No Posts", systemImage: "text.bubble",
                                       description: Text("This run has no posts."))
            }
        } else {
            let groups = groups
            let personas = store.personaByID
            if groups.isEmpty {
                ContentUnavailableView.search(text: search.isEmpty ? "round \(store.playhead)" : search)
            } else {
                ScrollViewReader { proxy in
                    List(selection: $store.selectedPostID) {
                        ForEach(groups) { group in
                            if scope == .playheadRound {
                                Section {
                                ForEach(group.events, id: \.content) { event in
                                        EventBanner(event: event)
                                            .selectionDisabled()
                                            .listRowBackground(Color.clear)
                                            .listRowSeparator(.hidden)
                                            .listRowInsets(EdgeInsets(top: 8, leading: Theme.space4, bottom: 6, trailing: Theme.space4))
                                    }
                                    ForEach(group.rows) { row in
                                        FeedPostRow(row: row, persona: personas[row.post.authorID], isSelected: store.selectedPostID == row.post.id) { agentID in
                                            store.selectedAgentID = agentID
                                        } selectPost: { postID in
                                            store.selectedPostID = postID
                                        }
                                        .tag(row.post.id)
                                        .listRowBackground(store.selectedPostID == row.post.id ? Theme.accentSoft : Color.clear)
                                        .listRowSeparatorTint(Theme.seam)
                                        .listRowInsets(EdgeInsets(top: 0, leading: Theme.space4, bottom: 0, trailing: Theme.space4))
                                    }
                                }
                            } else {
                                Section {
                                ForEach(group.events, id: \.content) { event in
                                        EventBanner(event: event)
                                            .selectionDisabled()
                                            .listRowBackground(Color.clear)
                                            .listRowSeparator(.hidden)
                                            .listRowInsets(EdgeInsets(top: 8, leading: Theme.space4, bottom: 6, trailing: Theme.space4))
                                    }
                                    ForEach(group.rows) { row in
                                        FeedPostRow(row: row, persona: personas[row.post.authorID], isSelected: store.selectedPostID == row.post.id) { agentID in
                                            store.selectedAgentID = agentID
                                        } selectPost: { postID in
                                            store.selectedPostID = postID
                                        }
                                        .tag(row.post.id)
                                        .listRowBackground(store.selectedPostID == row.post.id ? Theme.accentSoft : Color.clear)
                                        .listRowSeparatorTint(Theme.seam)
                                        .listRowInsets(EdgeInsets(top: 0, leading: Theme.space4, bottom: 0, trailing: Theme.space4))
                                    }
                                } header: {
                                    RoundHeader(round: group.round, count: group.rows.count,
                                                sentiment: RunUI.roundSentiment(group.round, in: store))
                                        .id("round-\(group.round)")
                                }
                            }
                        }
                    }
                    .listStyle(.plain)
                    .scrollContentBackground(.hidden)
                    .background(Theme.card)
                    .alternatingRowBackgrounds(.disabled)
                    .onChange(of: store.selectedPostID) { _, id in
                        guard let id else { return }
                        Task { @MainActor in
                            // Let the list lay out rows that just became visible (scope / playhead) first.
                            try? await Task.sleep(for: .milliseconds(60))
                            withAnimation(.snappy) { proxy.scrollTo(id, anchor: .top) }
                        }
                    }
                    .onChange(of: store.playhead) { _, round in
                        // The round's header (and its pinned event card) goes to the top, not its first post.
                        guard scope == .throughPlayhead, groups.contains(where: { $0.round == round }) else { return }
                        proxy.scrollTo("round-\(round)", anchor: .top)
                    }
                    .task {
                        try? await Task.sleep(for: .milliseconds(60))
                        if let id = store.selectedPostID {
                            proxy.scrollTo(id, anchor: .center)
                        } else if scope == .throughPlayhead, groups.contains(where: { $0.round == store.playhead }) {
                            proxy.scrollTo("round-\(store.playhead)", anchor: .top)
                        }
                    }
                }
            }
        }
    }
}

// MARK: - rows

private struct RoundHeader: View {
    let round: Int
    let count: Int
    let sentiment: Double?

    var body: some View {
        HStack(spacing: 8) {
            Text(round == 0 ? "Opening posts" : "Round \(round)")
                .font(Theme.ui(12, .semibold))
                .foregroundStyle(Theme.text)
            Text("\(count) posts").font(Theme.small).foregroundStyle(Theme.muted)
            Spacer()
            if let sentiment {
                Text("mean").font(Theme.small).foregroundStyle(Theme.muted)
                SentimentValue(score: sentiment, size: 11)
            }
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 2)
        .frame(maxWidth: .infinity)
        .background(Theme.card)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// The injected event, pinned at the top of its round: the only amber block in the feed.
private struct EventBanner: View {
    let event: GodEvent

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "bolt.fill").font(.system(size: 10))
                Text("Event, round \(event.round)")
                    .font(Theme.ui(12, .semibold))
            }
            .foregroundStyle(Theme.event)
            Text(event.content)
                .font(Theme.body)
                .foregroundStyle(Theme.text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.eventSoft, in: RoundedRectangle(cornerRadius: Theme.radius + 2))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Event injected in round \(event.round): \(event.content)")
    }
}

private struct FeedPostRow: View {
    let row: FeedView.Row
    let persona: Persona?
    let isSelected: Bool
    let selectAgent: (String) -> Void
    let selectPost: (Int) -> Void

    var body: some View {
        let post = row.post
        HStack(alignment: .top, spacing: 0) {
            if row.depth > 0 {
                Rectangle()
                    .fill(Theme.seamStrong)
                    .frame(width: 1)
                    .padding(.leading, CGFloat(row.depth - 1) * 14 + 6)
                    .padding(.trailing, 8)
            }
            Button { selectAgent(post.authorID) } label: {
                avatar(post)
            }
            .buttonStyle(.plain)
            .padding(.top, 10)
            .padding(.trailing, 10)
            .help("Show @\(post.authorHandle)")
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    VStack(alignment: .leading, spacing: 0) {
                        if let persona {
                            Text(persona.name).font(Theme.ui(13, .semibold)).foregroundStyle(Theme.text).lineLimit(1)
                        }
                        RunHandleChip(handle: post.authorHandle, sentiment: nil) { selectAgent(post.authorID) }
                    }
                    if post.repostOf != nil {
                        Label("reposted", systemImage: "arrow.2.squarepath")
                            .font(Theme.caption)
                            .foregroundStyle(Theme.repost)
                    }
                    Spacer(minLength: 4)
                    if post.sentiment != nil { SentimentValue(score: post.sentiment, size: 11) }
                    Text("#\(post.id)")
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.muted)
                }

                if let parent = row.replyingTo {
                    Button { selectPost(parent.id) } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "arrow.turn.down.right").font(.system(size: 9))
                            Text("replying to @\(parent.authorHandle) #\(parent.id)")
                                .font(Theme.mono(11))
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                        .foregroundStyle(Theme.link)
                    }
                    .buttonStyle(.plain)
                    .help("Show the post this replies to")
                }

                if !post.content.isEmpty {
                    Text(post.content)
                        .font(Theme.body)
                        .foregroundStyle(Theme.text)
                        .lineSpacing(2)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }

                if let quoted = row.quoted {
                    Button { selectPost(quoted.id) } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("@\(quoted.authorHandle)  #\(quoted.id)")
                                .font(Theme.mono(11))
                                .foregroundStyle(Theme.repost)
                            Text(quoted.content)
                                .font(Theme.small)
                                .foregroundStyle(Theme.muted)
                                .lineLimit(4)
                                .multilineTextAlignment(.leading)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                        .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Quoted post by @\(quoted.authorHandle): \(quoted.content)")
                }

                // Only counters with something in them: a post nobody reacted to has no row of zeros.
                if post.replies + post.reposts + post.likes + post.dislikes > 0 {
                    HStack(spacing: 16) {
                        if post.replies > 0 { counter("arrow.turn.up.left", post.replies, "replies") }
                        if post.reposts > 0 { counter("arrow.2.squarepath", post.reposts, "reposts") }
                        if post.likes > 0 { counter("heart", post.likes, "likes") }
                        if post.dislikes > 0 { counter("hand.thumbsdown", post.dislikes, "dislikes") }
                    }
                    .padding(.top, 1)
                }
            }
            .padding(.vertical, 9)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Post \(post.id) by @\(post.authorHandle)")
    }

    @ViewBuilder
    private func avatar(_ post: PostOut) -> some View {
        if let persona {
            AvatarView(persona: persona, size: 28)
        } else {
            AvatarView(identity: AvatarIdentity(id: post.authorID, handle: post.authorHandle, name: post.authorHandle, isOrganisation: false), size: 28)
        }
    }

    /// Counts are quiet: colour is reserved for sentiment and events, the symbol and the label say which kind it is.
    private func counter(_ symbol: String, _ value: Int, _ noun: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: symbol).font(.system(size: 10)).foregroundStyle(Theme.muted)
            Text("\(value)").font(Theme.mono(11)).foregroundStyle(Theme.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(value) \(noun)")
    }
}
