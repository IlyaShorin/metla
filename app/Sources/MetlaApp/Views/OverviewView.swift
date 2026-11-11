import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The Overview: the partial-analysis banner, the disk card, what can be
/// freed, the largest folders and the bytes by data type.
struct OverviewView: View {
    let model: AppModel

    var body: some View {
        ScrollView {
            if let data = model.overviewData {
                VStack(alignment: .leading, spacing: 14) {
                    if let banner = data.banner {
                        BannerView(banner: banner, strings: model.strings) {
                            Task { await model.show(.folders) }
                        }
                    }
                    if let disk = data.disk {
                        DiskCard(disk: disk, strings: model.strings)
                    }
                    HStack(alignment: .top, spacing: 14) {
                        FreedCard(data: data, model: model)
                        LargestCard(data: data, model: model)
                    }
                    KindsCard(data: data, strings: model.strings)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
            } else {
                ProgressView().padding(40)
            }
        }
    }
}

/// A rounded card with a light border, as the mock-ups draw them.
struct Card<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) { content }
            .padding(.horizontal, 16)
            .padding(.vertical, 14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 10).fill(.background))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
    }
}

/// A thin bar filled to `share`.
struct ShareBar: View {
    let share: Double
    var tint: Color = .secondary

    var body: some View {
        GeometryReader { space in
            ZStack(alignment: .leading) {
                Capsule().fill(.quaternary)
                Capsule().fill(tint).frame(width: max(0, space.size.width * share))
            }
        }
        .frame(height: 8)
        .accessibilityHidden(true)
    }
}

struct BannerView: View {
    let banner: OverviewData.Banner
    let strings: Strings
    let showFolders: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            switch banner {
            case .scanning:
                ProgressView().controlSize(.small)
                Text(strings.text("overview.scanning"))
                Spacer()
            case .refreshing:
                ProgressView().controlSize(.small)
                Text(strings.text("overview.refreshing"))
                Spacer()
            case .partial(let denied, let missing):
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    .accessibilityHidden(true)
                (Text(strings.text("overview.partialTitle")).bold() + Text(" ")
                    + Text(strings.format("overview.partialDenied", Int(denied)))
                    + Text(missing > 0 ? " " + strings.format("overview.partialOther", Int(missing)) : "")
                    + Text(" ") + Text(strings.text("overview.partialLowerBound")))
                    .fixedSize(horizontal: false, vertical: true)
                Spacer()
                Button(strings.text("overview.showFolders"), action: showFolders)
                Button(strings.text("overview.openAccess")) { SystemSettings.openFullDiskAccess() }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 8).fill(.orange.opacity(0.12)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.orange.opacity(0.4)))
        .accessibilityElement(children: .contain)
    }
}

/// The disk: name, used of total, free (red when under a tenth), and a
/// stacked bar with its legend.
struct DiskCard: View {
    let disk: OverviewData.Disk
    let strings: Strings

    var body: some View {
        Card {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(disk.title).font(.title3.weight(.semibold))
                    .lineLimit(1).truncationMode(.middle)
                if disk.folder {
                    Text(strings.format("overview.analysed", Format.bytes(disk.analysed, strings)))
                        .font(.title3.weight(.semibold))
                    Spacer()
                    Text(strings.format("overview.onVolume", disk.volume, Format.bytes(disk.free, strings)))
                        .foregroundStyle(disk.low ? .red : .secondary)
                } else {
                    Text(strings.format("overview.used", Format.bytes(disk.used, strings), Format.bytes(disk.total, strings)))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Text(strings.format("overview.freeOf", Format.bytes(disk.free, strings)))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(disk.low ? .red : .primary)
                }
            }
            GeometryReader { space in
                HStack(spacing: 1) {
                    ForEach(Array(disk.segments.enumerated()), id: \.element.id) { index, part in
                        Rectangle()
                            .fill(Palette.segment(index, count: disk.segments.count))
                            .frame(width: max(part.bytes > 0 ? 2 : 0, space.size.width * part.share))
                    }
                }
            }
            .frame(height: 22)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .accessibilityLabel(strings.text("overview.diskBar"))
            Flow(spacing: 18, lineSpacing: 6) {
                ForEach(Array(disk.segments.enumerated()), id: \.element.id) { index, part in
                    HStack(spacing: 6) {
                        RoundedRectangle(cornerRadius: 3)
                            .fill(Palette.segment(index, count: disk.segments.count))
                            .frame(width: 9, height: 9)
                            .accessibilityHidden(true)
                        Text(part.name)
                        Text(Format.bytes(part.bytes, strings)).foregroundStyle(.secondary)
                    }
                    .fixedSize()
                }
            }
            .font(.callout)
        }
    }
}

/// "Can be freed ≈ N": the estimate, one line about clones, and a row
/// per category with how it is removed.
struct FreedCard: View {
    let data: OverviewData
    let model: AppModel

    var body: some View {
        let strings = model.strings
        Card {
            HStack(alignment: .firstTextBaseline) {
                Text(strings.text("overview.canBeFreed")).font(.headline)
                Spacer()
                Text("\u{2248} " + Format.bytes(data.freed, strings) + (data.growing ? "\u{2026}" : ""))
                    .font(.title2.weight(.semibold))
                    .monospacedDigit()
            }
            Text(strings.text("overview.estimate"))
                .font(.caption).foregroundStyle(.secondary)
            if data.trashed > 0 {
                Text(strings.format("overview.toTrash", Format.bytes(data.trashed, strings)))
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text(strings.text("overview.rowsNote"))
                .font(.caption).foregroundStyle(.secondary)
            ForEach(Array(data.categories.enumerated()), id: \.element.id) { index, part in
                Button {
                    if let category = part.category {
                        Task { await model.show(.category(category)) }
                    }
                } label: {
                    // The bar takes what the name and the size leave: how
                    // the category goes and what was found sit below.
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 8) {
                            Text(part.name).lineLimit(1)
                                .frame(width: 128, alignment: .leading)
                            ShareBar(share: part.share, tint: Palette.category(index))
                                .frame(maxWidth: .infinity)
                            Text(Format.bytes(part.bytes, strings)).monospacedDigit()
                                .frame(width: 72, alignment: .trailing)
                        }
                        HStack(spacing: 6) {
                            Text(part.note ?? "")
                            if let found = part.found {
                                Text(strings.format("overview.found", Format.bytes(found, strings)))
                            }
                        }
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(
                    "\(part.name), \(Format.bytes(part.bytes, strings)), \(part.note ?? "")"
                        + (part.found.map { ", " + strings.format("overview.found", Format.bytes($0, strings)) } ?? ""))
            }
        }
    }
}

/// The largest folders, with a note that size is not a sign of junk.
struct LargestCard: View {
    let data: OverviewData
    let model: AppModel

    var body: some View {
        let strings = model.strings
        Card {
            HStack(alignment: .firstTextBaseline) {
                Text(strings.text("overview.largest")).font(.headline)
                Spacer()
                Button(strings.text("overview.allFolders")) {
                    Task { await model.show(.folders) }
                }
                .buttonStyle(.link)
            }
            Text(strings.text("overview.notJunk"))
                .font(.caption).foregroundStyle(.secondary)
            ForEach(data.largest) { part in
                HStack(spacing: 10) {
                    // The path has the row's free width; it truncates in the
                    // middle only when it does not fit.
                    Text(part.name)
                        .font(.system(.callout, design: .monospaced))
                        .lineLimit(1).truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .layoutPriority(1)
                        .help(part.id)
                    ShareBar(share: part.share).frame(width: 72)
                    Text(Format.bytes(part.bytes, strings) + (data.growing ? "\u{2026}" : ""))
                        .monospacedDigit().frame(width: 72, alignment: .trailing)
                }
                .accessibilityElement(children: .combine)
            }
        }
    }
}

/// Bytes per kind of data.
struct KindsCard: View {
    let data: OverviewData
    let strings: Strings

    var body: some View {
        Card {
            Text(strings.text("overview.byKind")).font(.headline)
            HStack(spacing: 18) {
                ForEach(data.kinds) { part in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(part.name).font(.caption).foregroundStyle(.secondary)
                        Text(Format.bytes(part.bytes, strings)).monospacedDigit()
                        ShareBar(share: part.share).frame(width: 80)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }
}

/// System colours for the categories and the disk bar's other segments.
enum Palette {
    static let categories: [Color] = [.blue, .purple, .orange, .teal, .green, .brown]

    static func category(_ index: Int) -> Color {
        categories[index % categories.count]
    }

    /// The disk bar: the three quick categories, the rest of the
    /// cleanable, other data, free.
    static func segment(_ index: Int, count: Int) -> Color {
        switch count - index {
        case 1: Color(nsColor: .quaternaryLabelColor)
        case 2: Color(nsColor: .systemGray)
        case 3: .green
        default: category(index)
        }
    }
}

/// Lays its views out left to right, starting a new line when one does
/// not fit: the disk legend, whatever the language.
struct Flow: Layout {
    var spacing: CGFloat
    var lineSpacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        let places = place(subviews, in: width)
        let height = places.map { $0.maxY }.max() ?? 0
        let used = places.map { $0.maxX }.max() ?? 0
        return CGSize(width: proposal.width ?? used, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for (view, frame) in zip(subviews, place(subviews, in: bounds.width)) {
            view.place(at: CGPoint(x: bounds.minX + frame.minX, y: bounds.minY + frame.minY), proposal: .unspecified)
        }
    }

    private func place(_ subviews: Subviews, in width: CGFloat) -> [CGRect] {
        var frames: [CGRect] = []
        var x: CGFloat = 0
        var y: CGFloat = 0
        var line: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width {
                x = 0
                y += line + lineSpacing
                line = 0
            }
            frames.append(CGRect(origin: CGPoint(x: x, y: y), size: size))
            x += size.width + spacing
            line = max(line, size.height)
        }
        return frames
    }
}
