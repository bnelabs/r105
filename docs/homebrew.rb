class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.4.0/r105-macos-arm64.tar.gz"
      sha256 "d3becf7044884ad14201019d0573da6b491215e611d1e1f46a768f5245c3f0db"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.4.0/r105-macos-x86_64.tar.gz"
      sha256 "387a4c7e2988f12fbf38f316a4d85f22dfc6c97f97675aaaf8997bb2bdb4f7f9"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.4.0/r105-linux-aarch64.tar.gz"
      sha256 "4c600d02cf133027fed28e1c732ebd8106d492c4500b29e30f38f175b7e914b0"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.4.0/r105-linux-x86_64.tar.gz"
      sha256 "b54c8c0987c949eb134b3f3b313fe50bb52b626e5018f000fb4ed004a2b01813"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
