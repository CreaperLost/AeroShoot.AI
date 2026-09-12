// Test entry-point for the mic gain contract suite. Wired into the macOS
// Swift build via the `MIC_GAIN_TESTS` Swift compiler flag; see the
// project's `script/test-mic-gain.sh` (or equivalent) for invocation.
@main
struct MicGainContractTests {
  static func main() throws {
    try MicGainContracts.run()
  }
}
