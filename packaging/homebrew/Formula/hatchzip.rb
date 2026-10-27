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
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.7/hatchzip-aarch64-macos.tar.gz"
      sha256 "b23b888c7f5608e99f75680b8918789f9f9e015b50087933a8fec5c5d9f87a8c"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.7/hatchzip-x86_64-macos.tar.gz"
      sha256 "63483dd17a38759d1cdbff5f52f4f38c6566f44396596415376eecbbf6b29be1"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.7/hatchzip-aarch64-linux.tar.gz"
      sha256 "15e6fc9c685931b09caa9b5e8da36f4d609652399f151d533624f8e60e033853"
    end
    on_intel do
      url "https://github.com/xevrion/hatchzip/releases/download/v0.1.7/hatchzip-x86_64-linux.tar.gz"
      sha256 "ded237934470a93f380eddb05b70d0d060a009ef9491aa915376b3bd121bc18e"
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
