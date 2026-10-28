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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.8/hatchzip-aarch64-macos.tar.gz"
      sha256 "4d393d7b262345d1b373d58b600ca137b42dfa7e069a67d739eabb8c83751a12"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.8/hatchzip-x86_64-macos.tar.gz"
      sha256 "8754f141343630fc8ca3efb7610899d8994e5b4e6b7ffb5655c91c51f2c8de5c"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.8/hatchzip-aarch64-linux.tar.gz"
      sha256 "8f1a4d18248bd35e07074890d673769c0a173d9266db26dfcb1ba6cf4fa02468"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.8/hatchzip-x86_64-linux.tar.gz"
      sha256 "9c2e734dde74ea822979e6ddb071e7271b4d5fd924e4f3cf48b778f8a70da301"
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
