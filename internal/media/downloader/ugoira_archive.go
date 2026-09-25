package downloader

import (
	"archive/zip"
	"context"
	"errors"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"strings"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader/filename"
	filereplace "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/replace"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

// 本地失败 code。SDK Reason 覆盖 not_ugoira / ugoira_archive_missing /
// ugoira_frame_mismatch；这两个只在本机产物校验时产生。
const (
	CodeInsecureFrameName = "insecure_frame_name"
	CodeWriteFailed       = "write_failed"
)

// UgoiraFrameReport 汇总声明帧与 zip 实际 entry 的差异。Undeclared 按 zip 顺序，
// Missing 按声明顺序；actual 不包含目录 entry、__MACOSX/ 与重复名。
type UgoiraFrameReport struct {
	Declared   int      `json:"declared"`
	Actual     int      `json:"actual"`
	Undeclared []string `json:"undeclared"`
	Missing    []string `json:"missing"`
}

// ClassifiedDownloadError 携带稳定的失败 code 与随失败发布的产物路径，让
// CLI/MCP 无需解析错误文本即可分类和定位隔离产物。
type ClassifiedDownloadError struct {
	Code    string
	Path    string
	Missing []string
	Err     error
}

func (e *ClassifiedDownloadError) Error() string {
	if e.Err != nil {
		return e.Err.Error()
	}
	return e.Code
}

func (e *ClassifiedDownloadError) Unwrap() error { return e.Err }

func classifiedError(code, artifactPath string, missing []string, err error) error {
	return &ClassifiedDownloadError{Code: code, Path: artifactPath, Missing: missing, Err: err}
}

// downloadUgoiraArchive 保存上游 zip 原文并校验声明帧。成功时发布到最终路径；
// 缺失帧、重复名、非法 entry、zip 损坏或空档案会隔离到 .quarantine 并返回
// 带 code/path 的失败，此时不产生 item（隔离产物不是已完成的作品）。
func (m *Manager) downloadUgoiraArchive(ctx context.Context, artwork pixiv.Artwork, base string) (DownloadedArtwork, []DownloadWarning, error) {
	out := DownloadedArtwork{
		IllustID: artwork.ID,
		Title:    artwork.Title,
		Author:   artwork.User.Name,
		Type:     string(pixiv.ArtworkKindUgoira),
	}
	basename, warnings, err := m.ugoiraBasename(artwork)
	if err != nil {
		return out, warnings, err
	}
	metadata, err := m.client.UgoiraMetadata(ctx, pixiv.UgoiraMetadataRequest{ArtworkID: artwork.ID})
	if err != nil {
		return out, warnings, err
	}
	archive := selectUgoiraArchive(metadata)
	if archive == nil || archive.Resource.URL == "" {
		return out, warnings, classifiedError(string(sdk.UgoiraArchiveMissing), "", nil, fmt.Errorf("ugoira %d has no downloadable archive", artwork.ID))
	}
	zipFile, err := os.CreateTemp(base, "ugoira-*.zip")
	if err != nil {
		return out, warnings, classifiedError(CodeWriteFailed, "", nil, err)
	}
	tempPath := zipFile.Name()
	if err := zipFile.Close(); err != nil {
		_ = os.Remove(tempPath)
		return out, warnings, classifiedError(CodeWriteFailed, "", nil, err)
	}
	published := false
	defer func() {
		if !published {
			_ = os.Remove(tempPath)
		}
	}()
	saved, err := m.saveResource(ctx, archive.Resource.Ref, tempPath)
	if err != nil {
		return out, warnings, err
	}
	report, failureCode, failureErr := inspectUgoiraArchive(tempPath, metadata.Frames)
	out.Quality = string(archive.Quality)
	out.Frames = metadata.Frames
	out.FrameReport = report
	if failureCode != "" {
		quarantinePath := m.quarantinePath(artwork.ID)
		if err := os.MkdirAll(filepath.Dir(quarantinePath), 0o755); err != nil {
			return out, warnings, classifiedError(CodeWriteFailed, "", nil, err)
		}
		if err := filereplace.ReplaceFile(tempPath, quarantinePath); err != nil {
			if filereplace.MustPreserveReplacementSource(err) {
				published = true
			}
			return out, warnings, classifiedError(CodeWriteFailed, "", nil, err)
		}
		published = true
		return out, warnings, classifiedError(failureCode, quarantinePath, report.Missing, failureErr)
	}
	finalPath := filepath.Join(base, basename+".zip")
	if err := filereplace.ReplaceFile(tempPath, finalPath); err != nil {
		if filereplace.MustPreserveReplacementSource(err) {
			published = true
		}
		return out, warnings, classifiedError(CodeWriteFailed, "", nil, err)
	}
	published = true
	out.Files = append(out.Files, DownloadedFile{Path: finalPath, Page: 1, Bytes: saved.Size})
	if len(report.Undeclared) > 0 {
		warnings = append(warnings, DownloadWarning{
			IllustID: artwork.ID,
			Type:     string(pixiv.ArtworkKindUgoira),
			Message:  "ugoira archive contains undeclared frames",
		})
	}
	return out, warnings, nil
}

// ugoiraBasename 复用 filename 模板并保留既有回退 warning，供 GIF/APNG 与 zip
// 两条路径共用。
func (m *Manager) ugoiraBasename(artwork pixiv.Artwork) (string, []DownloadWarning, error) {
	data := filenameData(artwork)
	basename, generationErr := filename.GenerateChecked(data, 0, m.filenameTemplate)
	if generationErr == nil && basename != "" {
		return basename, nil, nil
	}
	fallback, fallbackErr := filename.GenerateChecked(data, 0, "")
	if fallbackErr != nil {
		return "", nil, fmt.Errorf("ugoira filename fallback failed: %w", fallbackErr)
	}
	if fallback == "" {
		return "", nil, errors.New("ugoira filename fallback produced an empty name")
	}
	message := "ugoira filename template failed; using default filename"
	if generationErr == nil {
		message = "ugoira filename template produced an empty name; using default filename"
	}
	return fallback, []DownloadWarning{{
		IllustID: artwork.ID,
		Type:     string(pixiv.ArtworkKindUgoira),
		Message:  message,
	}}, nil
}

func (m *Manager) quarantinePath(id int64) string {
	return filepath.Join(m.DownloadPath(), ".quarantine", fmt.Sprintf("%d.zip", id))
}

// inspectUgoiraArchive 只读取 zip 中央目录，不展开帧内容。返回空 code 表示可
// 以发布到最终路径；否则调用方隔离产物。
func inspectUgoiraArchive(zipPath string, declared []pixiv.UgoiraFrame) (*UgoiraFrameReport, string, error) {
	reader, err := zip.OpenReader(zipPath)
	if err != nil {
		return &UgoiraFrameReport{Declared: len(declared)}, string(sdk.UgoiraArchiveMissing), fmt.Errorf("ugoira archive cannot be read: %w", err)
	}
	defer reader.Close()
	declaredSet := make(map[string]struct{}, len(declared))
	for _, frame := range declared {
		declaredSet[frame.Filename] = struct{}{}
	}
	seen := make(map[string]struct{})
	var actual []string
	for _, file := range reader.File {
		name := file.Name
		if strings.HasSuffix(name, "/") || strings.HasPrefix(name, "__MACOSX/") {
			continue
		}
		if !safeUgoiraEntryName(name) {
			return &UgoiraFrameReport{Declared: len(declared), Actual: len(actual)}, CodeInsecureFrameName, errors.New("ugoira archive contains an unsafe entry name")
		}
		if _, duplicate := seen[name]; duplicate {
			return &UgoiraFrameReport{Declared: len(declared), Actual: len(actual)}, string(sdk.UgoiraFrameMismatch), fmt.Errorf("ugoira archive contains duplicate entry %q", name)
		}
		seen[name] = struct{}{}
		actual = append(actual, name)
	}
	report := &UgoiraFrameReport{Declared: len(declared), Actual: len(actual)}
	for _, name := range actual {
		if _, ok := declaredSet[name]; !ok {
			report.Undeclared = append(report.Undeclared, name)
		}
	}
	for _, frame := range declared {
		if _, ok := seen[frame.Filename]; !ok {
			report.Missing = append(report.Missing, frame.Filename)
		}
	}
	if len(report.Missing) > 0 {
		return report, string(sdk.UgoiraFrameMismatch), fmt.Errorf("ugoira archive is missing %d declared frames", len(report.Missing))
	}
	if len(actual) == 0 {
		return report, string(sdk.UgoiraArchiveMissing), errors.New("ugoira archive has no frames")
	}
	return report, "", nil
}

func fileSize(path string) int64 {
	info, err := os.Stat(path)
	if err != nil {
		return 0
	}
	return info.Size()
}

// safeUgoiraEntryName 与 SDK 的 safeArtworkArchiveFilename 保持同一判定：拒绝
// 绝对路径、反斜杠、. 与 .. 前缀，避免消费者按 entry 名展开时逃出目标目录。
func safeUgoiraEntryName(name string) bool {
	if name == "" || strings.ContainsAny(name, "\\") || path.IsAbs(name) {
		return false
	}
	cleaned := path.Clean(name)
	return cleaned == name && cleaned != "." && !strings.HasPrefix(cleaned, "..")
}
