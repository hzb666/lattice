# 测试样本

- `fields.cdxml`：手工构造的回归样本，覆盖两页、Group、原子/键、ObjectTag、箭头、曲线、未知对象、中文和跨样式动态字段；不是 ChemDraw 导出样本。
- `rdkit/mol1.cdxml`：取自 RDKit 提交 `79045367d04b6f9d11e29ae73f3e1d6b210d9641` 的 `Code/GraphMol/FileParsers/mol1.cdxml`，原文标记 `CreationProgram="ChemDraw 6.0.1"`。保留绘图内容，统一了换行和原文件空白行；许可证见 `rdkit/LICENSE`。
- `rdkit/Cp-.cdxml`：取自同一提交的 `rdkit/Chem/test_data/Cp-.cdxml`，同为 ChemDraw 6.0.1 生成，带外部 DTD 声明。保留绘图内容，统一了换行和原文件空白行；适用同一许可证。

真实来源样本用于验证可读取性，不代表当前 ChemDraw 版本的保存或视觉保真已经验证。
