# For a later homebrew-core submission, which takes only formulae built from
# source. Only the command line tool: the desktop app stays a cask.
class Hatchzip < Formula
  desc "Extract huge archives in place, needing free space only for the contents"
  homepage "https://hatchzip.dev"
  url "https://github.com/xevrion/hatchzip/archive/refs/tags/v0.1.5.tar.gz"
  sha256 "54d4630bcdc7d2c1177814d17f2de7b1f545a1e06de116e3fa2b8f3899f0003e"
  license any_of: ["MIT", "Apache-2.0"]
  head "https://github.com/xevrion/hatchzip.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/cli")
    bin.install_symlink "hatchzip" => "hz"

    generate_completions_from_executable(bin/"hatchzip", "completions")
    (man1/"hatchzip.1").write Utils.safe_popen_read(bin/"hatchzip", "man")
    man1.install_symlink "hatchzip.1" => "hz.1"
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
