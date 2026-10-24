# The command line tool from the prebuilt release tarballs, for the tap
# xevrion/homebrew-tap. The desktop app is the cask in ../Casks.
class Hatchzip < Formula
  desc "Extract huge archives in place, needing free space only for the contents"
  homepage "https://hatchzip.dev"
  license any_of: ["MIT", "Apache-2.0"]

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.3/hatchzip-aarch64-macos.tar.gz"
      sha256 "d5afa6ef14ee97d2df1740bd4eb8eeced2b83855b24ceb4b1d933d1a9c90d1ab"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.3/hatchzip-x86_64-macos.tar.gz"
      sha256 "6ac8bb9ec75b199f7161213fc2b1ddc4c16b0d68e44174e8b04074ebd26a2973"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.3/hatchzip-aarch64-linux.tar.gz"
      sha256 "1b485355a986889c1c4b8b8c62670eaa8037a5aa9e528c4780c91efb1fd7961d"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.3/hatchzip-x86_64-linux.tar.gz"
      sha256 "08ca9469878ce0ab14e275d4d4eb880e04cd57fa79097b0b4e649392011cc0fa"
    end
  end

  def install
    bin.install "hatchzip"
    bin.install_symlink "hatchzip" => "hz"
    man1.install "man/hatchzip.1"
    man1.install_symlink "hatchzip.1" => "hz.1"
    bash_completion.install "completions/hatchzip.bash" => "hatchzip"
    zsh_completion.install "completions/_hatchzip"
    fish_completion.install "completions/hatchzip.fish"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hatchzip --version")

    # A real extraction: the file comes out and the archive is gone.
    (testpath/"hello.txt").write "hatched\n"
    system "tar", "-cf", "test.tar", "hello.txt"
    rm "hello.txt"
    system bin/"hz", "extract", "--yes", "--keep-free", "0", "test.tar"
    assert_equal "hatched\n", (testpath/"test/hello.txt").read
    refute_path_exists testpath/"test.tar"
  end
end
