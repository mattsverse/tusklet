# Release template: brew bump-cask-pr replaces the version and both
# checksums before this cask is committed to the tap.
cask "tusklet" do
  version "0.0.0"

  on_macos do
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"

    url "https://github.com/mattsverse/tusklet/releases/download/v#{version}/Tusklet_#{version}_aarch64.dmg"

    depends_on arch: :arm64

    app "Tusklet.app"
  end

  on_linux do
    sha256 "2222222222222222222222222222222222222222222222222222222222222222"

    url "https://github.com/mattsverse/tusklet/releases/download/v#{version}/tusklet_#{version}_x86_64.AppImage"

    depends_on arch: :x86_64

    app_image "tusklet_#{version}_x86_64.AppImage", target: "Tusklet.AppImage"
    binary "tusklet_#{version}_x86_64.AppImage", target: "tusklet"
  end

  name "Tusklet"
  desc "Native workspace for local PostgreSQL containers"
  homepage "https://github.com/mattsverse/tusklet"

  caveats <<~EOS
    Tusklet requires the Docker CLI and a running local Docker daemon.
    The macOS app is ad-hoc signed and is not notarized.
    The Linux AppImage requires glibc 2.39 or newer, FUSE 2, and a Vulkan-capable graphics driver.
  EOS
end
