class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-macos-arm64.tar.gz"
      sha256 "c9acd4811dccde035edb98f35101ac5f42f0742690e8b61163e290381b03b923"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-macos-x86_64.tar.gz"
      sha256 "b9ac15c4563aacc0d7f3b502cbaa1f0c6da901391b070f04f93a9f35a5700001"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-linux-aarch64.tar.gz"
      sha256 "0087a03828d9d23c07e814c018cbed54bdbc923cba602f7ef739ab768719980b"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-linux-x86_64.tar.gz"
      sha256 "4a71d3a370543204a746f2db7c848cad40d83eb19eb6e00637f5d497f67ec55d"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
