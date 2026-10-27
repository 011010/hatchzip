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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.6/hatchzip-aarch64-macos.tar.gz"
      sha256 "e3efd5e9812e23cde232dad4a762f313cfa5a369e1ab9ebaf0c46faf51af0454"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.6/hatchzip-x86_64-macos.tar.gz"
      sha256 "bf2e0d183e7e8e3daa3a28e2717ecf97fd31b814c82cef74c5c9eb75d5e124e6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.6/hatchzip-aarch64-linux.tar.gz"
      sha256 "2fa2ac2fce84c22bd73018e719fd9b9b114caca25fb993961c2463ce8cd264af"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.6/hatchzip-x86_64-linux.tar.gz"
      sha256 "47a2d2f5665ec0af220f4d4fb47ff3f3cbf699a91e206ecab45cf234247c3513"
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
