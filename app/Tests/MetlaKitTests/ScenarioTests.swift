import Foundation
import Testing
@testable import MetlaKit

/// The `--scenario` runs, each on a fixture of its own: they pass only
/// when the fixture ends exactly as the flow confirmed.
@MainActor
@Suite struct ScenarioTests {
    @Test(arguments: Scenarios.names)
    func aScenarioLeavesTheFixtureAsConfirmed(_ name: String) async {
        let settings = Fixture.settings(root: Fixture.make())
        let code = await Scenarios.run(name, settings: settings, strings: Strings(language: "en", region: "US"))
        #expect(code == 0, "\(name)")
    }

    @Test func aScenarioRefusesWhatIsNotAFixtureOrAName() async {
        var settings = Fixture.settings(root: Fixture.make())
        let strings = Strings(language: "en", region: "US")
        #expect(await Scenarios.run("nonsense", settings: settings, strings: strings) == 2)
        settings.isFixture = false
        #expect(await Scenarios.run("default-clean", settings: settings, strings: strings) == 2)
    }
}
