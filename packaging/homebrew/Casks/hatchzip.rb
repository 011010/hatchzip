# The desktop app from the release dmg, for the tap xevrion/homebrew-tap.
# The dmg also carries the command line tool and its man page inside the app
# (see packaging/macos/build-dmg.sh), so they are linked onto PATH too.
cask "hatchzip" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.1.2"
  sha256 arm:   "4c195df5381b07dd5df89410b04e18ebf64bfddd5eca8239c04e24d0cf0541f7",
         intel: "3758532360f292a7f24b7ce628f3132948a320294699e47854bf8f2755d9404f"

  url "https://github.com/xevrion/hatchzip/releases/download/v#{version}/HatchZip-#{arch}.dmg"
  name "HatchZip"
  desc "Extract huge archives in place, needing free space only for the contents"
  homepage "https://hatchzip.dev/"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :big_sur

  app "HatchZip.app"
  binary "#{appdir}/HatchZip.app/Contents/MacOS/hatchzip"
  binary "#{appdir}/HatchZip.app/Contents/MacOS/hatchzip", target: "hz"
  manpage "#{appdir}/HatchZip.app/Contents/Resources/man/man1/hatchzip.1"

  zap trash: [
    "~/Library/Application Support/HatchZip",
    "~/Library/Preferences/io.github.xevrion.hatchzip.plist",
    "~/Library/Saved Application State/io.github.xevrion.hatchzip.savedState",
  ]

  caveats <<~EOS
    HatchZip is signed ad hoc, not with a Developer ID, so macOS asks before
    the first launch. On macOS 15 and later, open HatchZip once, then click
    "Open Anyway" in System Settings > Privacy & Security. On macOS 14 and
    earlier, Control-click HatchZip in Applications and choose Open.
  EOS
end
