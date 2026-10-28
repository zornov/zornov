# Zornov's Homebrew cask. It lives in the zornov/homebrew-tap repository as
# Casks/zornov.rb, so people install with:
#   brew trust zornov/tap && brew install --cask zornov/tap/zornov
# (homebrew/cask itself takes self-submitted apps from 225 stars). Each
# release: set version, and sha256 from the release's DMG digest.
cask "zornov" do
  version "0.1.1"
  sha256 "baf74f822c25122d28b6818df859037175167f786154a662f0297809e959685f"

  url "https://github.com/zornov/zornov/releases/download/v#{version}/Zornov-#{version}-macos-aarch64.dmg",
      verified: "github.com/zornov/zornov/"
  name "Zornov"
  desc "Free, open-source film studio that AI assistants can drive over MCP"
  homepage "https://zornov.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Zornov.app"
  binary "#{appdir}/Zornov.app/Contents/MacOS/zornov"

  zap trash: [
    "~/Library/Application Support/Zornov",
    "~/Library/Caches/Zornov",
    "~/Library/Preferences/com.zornov.app.plist",
    "~/Library/Saved Application State/com.zornov.app.savedState",
  ]
end
