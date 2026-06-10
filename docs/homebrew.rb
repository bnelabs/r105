class Rova < Formula
  include Language::Python::Virtualenv

  desc "Rapid On-demand Virtual Assistant — rich terminal frontend for llama-router"
  homepage "https://github.com/bnelabs/rova"
  url "https://files.pythonhosted.org/packages/source/r/rova/rova-0.2.1.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"

  depends_on "python@3.12"

  def install
    virtualenv_install_with_resources
  end

  test do
    assert_match "rova #{version}", shell_output("#{bin}/rova --version")
  end
end
