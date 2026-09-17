class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.5.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-macos-arm64.tar.gz"
      sha256 "009913e3107ea05e451a3cb3cca12a0fef1f3ea240a9f30286934f57a14fb9bf"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-macos-x86_64.tar.gz"
      sha256 "554539bf3cdbd712c0488e6595a09de24c2b25c7a6925c1a7e66b2763b658230"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-linux-aarch64.tar.gz"
      sha256 "c189d801240fa7509b8fb80e4f3b86aaf59e7fcb125dc96029ff308c98e47419"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-linux-x86_64.tar.gz"
      sha256 "833df759457c78778354a4f1e90c7f2ae030ae71cfc1cb9d15050f3b21013865"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
