package download

import (
	"bytes"
	"encoding/json"
	"io"

	downloader "github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

// artifactRecord 是 download 成功产物的机器可读记录。ugoira 使用 Quality/Frames/
// FrameReport；直链资源只有 Kind/Path/Bytes，不属于 image-manager 契约。
type artifactRecord struct {
	ArtworkID   int64                         `json:"artwork_id,omitempty"`
	Kind        string                        `json:"kind"`
	Page        int                           `json:"page,omitempty"`
	Path        string                        `json:"path"`
	Bytes       int64                         `json:"bytes"`
	Quality     string                        `json:"quality,omitempty"`
	Frames      []reportFrame                 `json:"frames,omitempty"`
	FrameReport *downloader.UgoiraFrameReport `json:"frame_report,omitempty"`
}

type reportFrame struct {
	Filename          string `json:"filename"`
	DelayMilliseconds int    `json:"delay_milliseconds"`
}

// failureRecord 是一次作品失败的机器可读记录；code 为稳定的小写 snake 分类。
type failureRecord struct {
	ArtworkID int64         `json:"artwork_id,omitempty"`
	Error     *failureError `json:"error"`
}

type failureError struct {
	Code    string   `json:"code"`
	Path    string   `json:"path,omitempty"`
	Missing []string `json:"missing,omitempty"`
}

// writeDownloadReport 把一次下载结果写成 --json 数组或 --ndjson 流，成功与失败
// 记录同列。它只写 JSON；warning 仍由调用方写到 stderr。
func writeDownloadReport(out io.Writer, report downloader.DownloadReport, ndjson bool) error {
	records := make([]any, 0, len(report.Items)+len(report.Failures))
	for _, item := range report.Items {
		for _, file := range item.Files {
			records = append(records, artifactRecordFor(item, file))
		}
	}
	for _, failure := range report.Failures {
		records = append(records, failureRecordFor(failure))
	}
	if ndjson {
		encoder := json.NewEncoder(out)
		for _, record := range records {
			if err := encoder.Encode(record); err != nil {
				return err
			}
		}
		return nil
	}
	body, err := json.Marshal(records)
	if err != nil {
		return err
	}
	var buffered bytes.Buffer
	if err := json.Indent(&buffered, body, "", "  "); err != nil {
		return err
	}
	_, err = io.WriteString(out, buffered.String()+"\n")
	return err
}

func artifactRecordFor(item downloader.DownloadedArtwork, file downloader.DownloadedFile) artifactRecord {
	record := artifactRecord{
		ArtworkID: item.IllustID,
		Kind:      item.Type,
		Path:      file.Path,
		Bytes:     file.Bytes,
	}
	if item.Type == downloader.DownloadedResourceType {
		return record
	}
	if item.Type == "ugoira" {
		record.Quality = item.Quality
		record.Frames = reportFrames(item.Frames)
		record.FrameReport = item.FrameReport
		return record
	}
	record.Page = file.Page
	return record
}

func reportFrames(frames []pixiv.UgoiraFrame) []reportFrame {
	if len(frames) == 0 {
		return nil
	}
	converted := make([]reportFrame, 0, len(frames))
	for _, frame := range frames {
		converted = append(converted, reportFrame{Filename: frame.Filename, DelayMilliseconds: frame.DelayMilliseconds})
	}
	return converted
}

func failureRecordFor(failure downloader.DownloadFailure) failureRecord {
	code := failure.Code
	if code == "" {
		code = string(sdk.ReasonOf(failure.Cause))
	}
	if code == "" {
		code = "command_failed"
	}
	return failureRecord{
		ArtworkID: failure.IllustID,
		Error:     &failureError{Code: code, Path: failure.Path, Missing: failure.Missing},
	}
}
