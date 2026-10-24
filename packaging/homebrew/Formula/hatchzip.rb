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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.2/hatchzip-aarch64-macos.tar.gz"
      sha256 "7ce3df744771cba925661157e9690d9b1daadcac58d7ccec652aa6beef3753bf"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.2/hatchzip-x86_64-macos.tar.gz"
      sha256 "ba4bc628d283a8bc20279a9cc42ab19802030f90706d340c8579e740f191eba7"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.2/hatchzip-aarch64-linux.tar.gz"
      sha256 "62bf8cb63e1ceae62d4f0e422562d354986da452f308dd1cc249ddb32d155a8e"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.2/hatchzip-x86_64-linux.tar.gz"
      sha256 "ee7d838e8be7a8b65c792739f2b69d1008fddaf6a0be48e457eaef7e5253cc78"
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
