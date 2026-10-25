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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.4/hatchzip-aarch64-macos.tar.gz"
      sha256 "039e50805718ca008f724f59345d9faa83e38c40b84e5a6e3d6a78311f2e0397"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.4/hatchzip-x86_64-macos.tar.gz"
      sha256 "a4d547450b754ccd7c5c99ca32518d3d3e61f07eeec9d7f7266d507bf7222307"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.4/hatchzip-aarch64-linux.tar.gz"
      sha256 "e4e4f2406922976d176cdae28107789c737593a8f79331447b80dec00b251eb1"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.4/hatchzip-x86_64-linux.tar.gz"
      sha256 "1834f82a21c42649005c979bfa730d48de5d633967e350bd67b8d6b8cd7df6ef"
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
