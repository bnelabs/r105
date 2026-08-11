class R105 < Formula
  include Language::Python::Virtualenv
  desc "r105 — Beyond the prompt. Rich terminal AI assistant for any OpenAI-compatible backend."
  homepage "https://github.com/bnelabs/r105"
  url "https://files.pythonhosted.org/packages/source/r/r105/r105-0.4.0.tar.gz"
  sha256 "1f2a29b1a5f7a37434f8563c67d38b2847356f03b352823d3256a383e0ee307b"
  license "MIT"
  depends_on "python@3.12"
  def install
    virtualenv_install_with_resources
  end
  test do
    assert_match "r105 #{version}", shell_output("#{bin}/r105 --version")
  end
end
