use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use std::path::Path;
#[derive(Debug, Clone, Default)]
pub struct FabricInstaller;
impl FabricInstaller {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
#[async_trait::async_trait]
impl ModLoaderInstaller for FabricInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Fabric
    }
    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        crate::minecraft::loader::libraries::install(
            self.loader_type(),
            mc_version,
            loader_version,
            game_dir,
        )
        .await
    }
}
