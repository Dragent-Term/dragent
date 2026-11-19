# The phone's end-to-end encryption module (docs/design/e2ee.md §3.2, E4).
# Links DragentE2EEFFI.xcframework (dragent_e2ee through uniffi), built by
# mobile/scripts/build-e2ee-rust.sh ios. Run that before `pod install`.
Pod::Spec.new do |s|
  s.name           = 'DragentE2EE'
  s.version        = '0.1.0'
  s.summary        = 'Dragent end-to-end encryption (dragent_e2ee through uniffi)'
  s.description    = s.summary
  s.license        = { :type => 'Proprietary' }
  s.author         = 'Dragent'
  s.homepage       = 'https://dragent.dev'
  s.platforms      = { :ios => '16.4' }
  s.swift_version  = '5.9'
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  xcframework = File.join(__dir__, 'Frameworks', 'DragentE2EEFFI.xcframework')
  unless File.exist?(xcframework)
    raise "DragentE2EE: #{xcframework} is missing. Run `bash scripts/build-e2ee-rust.sh ios` in mobile/ first."
  end
  s.vendored_frameworks = 'Frameworks/DragentE2EEFFI.xcframework'
  s.source_files = '*.swift', 'Shared/*.swift', 'Generated/*.swift'
  s.frameworks = 'Security', 'LocalAuthentication'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
