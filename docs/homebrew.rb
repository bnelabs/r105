class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.5.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-macos-arm64.tar.gz"
      sha256 "cae3c5f31566aaa7869298a3a52b109dbb6c560c4fd9fa38be49bbee61e1e563"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-macos-x86_64.tar.gz"
      sha256 "6d11a39a062fed0273e5658c3fa249e9eda8b055cf2ba15d02657debe3b3d9d0"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-linux-aarch64.tar.gz"
      sha256 "124d44ba3a95617a9af7a4aa4d6dba41c8108551985d8710dcec0f777f680a31"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.5.1/r105-linux-x86_64.tar.gz"
      sha256 "861fe9fd379e7c473d09eeaf96249ea52d79fe3f3cc4166d8741d872ef1fe473"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
