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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.5/hatchzip-aarch64-macos.tar.gz"
      sha256 "a232e8e0fa36c9d5e1840ac470b9c9832a18315137615ca438f0b4515381bc9e"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.5/hatchzip-x86_64-macos.tar.gz"
      sha256 "516bcad3f0a4f0c57963578f9140daa510f2a080ca1b0372f0f4c035151ffbc4"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.5/hatchzip-aarch64-linux.tar.gz"
      sha256 "26844bb33c084f920e1a5b9abbce0bd3478d252e290a387c7af160c5f1162bd8"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.5/hatchzip-x86_64-linux.tar.gz"
      sha256 "fe7de6c5c859779b056dd816aaa1bd1791fa672f51c3db95ad5e9f4b33fd49d7"
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
