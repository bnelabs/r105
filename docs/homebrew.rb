class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  url "https://github.com/bnelabs/r105/archive/refs/tags/v1.0.0.tar.gz"
  version "1.0.0"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "build", "--release", "--locked"
    bin.install "target/release/r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
