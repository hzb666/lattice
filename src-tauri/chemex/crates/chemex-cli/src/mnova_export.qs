var sourcePath = __SOURCE_PATH__;
var outputPath = __OUTPUT_PATH__;
var requestedSpectrum = __SPECTRUM_INDEX__;

function exportSpectrum() {
	var sourceWindow = new DocumentWindow(Application.mainWindow.newWindow());
	try {
		if (!serialization.open(sourcePath)) {
			throw new Error("Mnova could not open the source file");
		}
		var selected;
		for (var pageIndex = 0; pageIndex < sourceWindow.pageCount() && !selected; ++pageIndex) {
			var page = new Page(sourceWindow.page(pageIndex));
			for (var itemIndex = 0; itemIndex < page.itemCount(); ++itemIndex) {
				var pageItem = new PageItem(page.item(itemIndex));
				if (pageItem.name !== "NMR Spectrum") continue;
				var spectrum = new NMRSpectrum(pageItem);
				var count = spectrum.specCount;
				if (!(count > 0)) count = 1;
				if (requestedSpectrum <= count) {
					selected = {
						page: page,
						itemIndex: itemIndex,
						spectrumIndex: requestedSpectrum - 1
					};
					break;
				}
				requestedSpectrum -= count;
			}
		}
		if (!selected) {
			throw new Error("Mnova spectrum index is out of range");
		}

		var document = Application.mainWindow.newWindow();
		try {
			document.deletePages(document.pages());
			document.clonePages(selected.page);
			var targetPage = new Page(document.page(0));
			var remove = [];
			var targetSpectrum;
			for (var targetIndex = 0; targetIndex < targetPage.itemCount(); ++targetIndex) {
				var targetRawItem = targetPage.item(targetIndex);
				var targetItem = new PageItem(targetRawItem);
				if (targetItem.name !== "NMR Spectrum") continue;
				if (targetIndex === selected.itemIndex) {
					targetSpectrum = new NMRSpectrum(targetItem);
					targetSpectrum.curSpecIndex = selected.spectrumIndex;
				} else {
					remove.push(targetRawItem);
				}
			}
			if (!targetSpectrum || !targetSpectrum.isValid()) {
				throw new Error("Selected Mnova item is not a valid NMR spectrum");
			}
			if (remove.length) targetPage.deleteItems(remove);
			if (!serialization.save(outputPath, "pdf")) throw new Error("Mnova could not export the selected spectrum to PDF");
			return true;
		} finally {
			document.close();
		}
	} finally {
		sourceWindow.close();
	}
}
