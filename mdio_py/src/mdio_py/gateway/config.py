import os
from pathlib import Path
from pydantic_settings import BaseSettings, SettingsConfigDict

# 1. 定位**运行时根**（mdio 的家）
#    唯一实现 = mdio_py._env.runtime_root()。
from mdio_py._env import mdio_home, runtime_root

_file = Path(__file__).resolve()          # 本文件路径（下面 MDIO_WORKSPACE_ROOT 默认值还要用）
HOME_ROOT = mdio_home()                   # 地盘（PROJECT_ROOT 的值）
PROJECT_ROOT = HOME_ROOT                  # 兼容别名：沿用 env_file 键名
RUNTIME_ROOT = runtime_root()             # 根 wt（genesis）的根

class AppSettings(BaseSettings):
    model_config = SettingsConfigDict(
        # 尝试读取根目录下的 .env
        env_file=HOME_ROOT / ".env",
        env_ignore_empty=True,
        extra="ignore"
    )

    MDIO_GATEWAY_HOST: str = "127.0.0.1"
    MDIO_GATEWAY_PORT: int = 8011
    MDIO_DEBUG: bool = True

    # 默认值使用绝对路径
    # 默认 wt 的根 = 运行时根（= 根 wt genesis 的根）：装态 ~/.mdio/genesis、源码态 <repo>/workspace
    MDIO_WORKSPACE_ROOT: Path = runtime_root()

    def model_post_init(self, __context):
        """初始化后的钩子：确保路径存在"""
        # 如果从 env 读入的是相对路径，将其转换为基于**地盘**的绝对路径
        if not self.MDIO_WORKSPACE_ROOT.is_absolute():
            self.MDIO_WORKSPACE_ROOT = (HOME_ROOT / self.MDIO_WORKSPACE_ROOT).resolve()

        # 自动创建不存在的文件夹
        if not self.MDIO_WORKSPACE_ROOT.exists():
            print(f"⚠️ [Config] Workspace not found, creating: {self.MDIO_WORKSPACE_ROOT}")
            self.MDIO_WORKSPACE_ROOT.mkdir(parents=True, exist_ok=True)
        else:
            print(f"✅ [Config] Workspace loaded: {self.MDIO_WORKSPACE_ROOT}")

settings = AppSettings()
